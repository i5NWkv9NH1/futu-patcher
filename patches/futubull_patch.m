#define _GNU_SOURCE
#import <Foundation/Foundation.h>
#import <AppKit/AppKit.h>
#import <objc/runtime.h>
#include <crt_externs.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <stdarg.h>
#include <pthread.h>
#include <mach-o/dyld.h>
#include <sys/stat.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <netdb.h>
#include <mach/mach.h>
#include <mach/vm_map.h>
#include <libkern/OSCacheControl.h>

// * 1. 路径隔离与沙盒底层重定向

// * 获取沙盒隔离目录，将腾讯 IMSDK 与本地化冗余垃圾移出 ~/Documents
static NSString *GetIsolatedDocumentsPath(void) {
    static NSString *isolatedPath = nil;
    static dispatch_once_t onceToken;
    dispatch_once(&onceToken, ^{
        NSString *home = NSHomeDirectory();
        isolatedPath = [home stringByAppendingPathComponent:@"Library/Application Support/Futubull/SandboxedDocuments"];
        // * 如果目录不存在则自动递归创建
        [[NSFileManager defaultManager] createDirectoryAtPath:isolatedPath withIntermediateDirectories:YES attributes:nil error:nil];
    });
    return isolatedPath;
}

// * 检查并自动重写含有 com_tencent_imsdk_data 或 LocalizedString 的垃圾路径
static const char *redirect_sandbox_path(const char *path, char *buf, size_t buf_len) {
    if (!path) return path;
    // ! 同时匹配腾讯 IMSDK 缓存以及 LocalizedString 残留文件夹
    const char *match = strstr(path, "com_tencent_imsdk_data");
    if (!match) {
        match = strstr(path, "LocalizedString");
    }
    if (match) {
        const char *iso = [GetIsolatedDocumentsPath() UTF8String];
        snprintf(buf, buf_len, "%s/%s", iso, match);
        return buf;
    }
    return path;
}

// * 声明 macOS dyld 符号拦截结构体
#define DYLD_INTERPOSE(_replacement,_replacee) \
   __attribute__((used)) static struct{ const void* replacement; const void* replacee; } _interpose_##_replacee \
   __attribute__ ((section ("__DATA,__interpose"))) = { (const void*)(unsigned long)&_replacement, (const void*)(unsigned long)&_replacee };

// * 拦截 mkdir，重定向创建目录行为
int my_mkdir(const char *path, mode_t mode) {
    char buf[1024];
    return mkdir(redirect_sandbox_path(path, buf, sizeof(buf)), mode);
}
DYLD_INTERPOSE(my_mkdir, mkdir);

// * 拦截 open，重定向文件打开与创建行为
int my_open(const char *path, int oflag, ...) {
    mode_t mode = 0;
    if (oflag & O_CREAT) {
        va_list args;
        va_start(args, oflag);
        mode = (mode_t)va_arg(args, int);
        va_end(args);
    }
    char buf[1024];
    const char *real_path = redirect_sandbox_path(path, buf, sizeof(buf));
    return (oflag & O_CREAT) ? open(real_path, oflag, mode) : open(real_path, oflag);
}
DYLD_INTERPOSE(my_open, open);

// * 拦截 access，重定向文件可达性探测
int my_access(const char *path, int amode) {
    char buf[1024];
    return access(redirect_sandbox_path(path, buf, sizeof(buf)), amode);
}
DYLD_INTERPOSE(my_access, access);

// * 拦截 stat，重定向文件状态查询
int my_stat(const char *path, struct stat *buf_stat) {
    char buf[1024];
    return stat(redirect_sandbox_path(path, buf, sizeof(buf)), buf_stat);
}
DYLD_INTERPOSE(my_stat, stat);

// * 拦截 Cocoa 标准文档目录获取，重定向到自定义沙盒目录
NSArray<NSString *> *my_NSSearchPathForDirectoriesInDomains(NSSearchPathDirectory directory, NSSearchPathDomainMask domainMask, BOOL expandTilde) {
    if (directory == NSDocumentDirectory) {
        return @[GetIsolatedDocumentsPath()];
    }
    return NSSearchPathForDirectoriesInDomains(directory, domainMask, expandTilde);
}
DYLD_INTERPOSE(my_NSSearchPathForDirectoriesInDomains, NSSearchPathForDirectoriesInDomains);

// * 2. 拦截埋点网络请求 (DNS 解析与直连 IP)

// * 拦截 DNS 域名解析，屏蔽埋点采集、崩溃分析与监控域名
int my_getaddrinfo(const char *hostname, const char *servname, const struct addrinfo *hints, struct addrinfo **res) {
    if (hostname) {
        // ! 拦截各类数据上报域名，直接返回找不到主机名
        if (strstr(hostname, "collect.") ||
            strstr(hostname, "apm.futu") ||
            strstr(hostname, "crashlytics") ||
            strstr(hostname, "firebase") ||
            strstr(hostname, "googleapis.com") ||
            strstr(hostname, "app-measurement")) {
            return EAI_NONAME;
        }
    }
    return getaddrinfo(hostname, servname, hints, res);
}
DYLD_INTERPOSE(my_getaddrinfo, getaddrinfo);

// * 拦截底层 socket connect 连接，阻断硬编码的直连上报 IP
int my_connect(int socket, const struct sockaddr *address, socklen_t address_len) {
    if (address && address->sa_family == AF_INET) {
        struct sockaddr_in *in = (struct sockaddr_in *)address;
        uint32_t ip = ntohl(in->sin_addr.s_addr);
        // ! 匹配富途已知的两组直连数据上报 IP: 42.193.128.62 (0x2AC1803E) 与 129.226.79.246 (0x81E24FF6)
        if (ip == 0x2AC1803E || ip == 0x81E24FF6) {
            errno = ECONNREFUSED;
            return -1;
        }
    }
    return connect(socket, address, address_len);
}
DYLD_INTERPOSE(my_connect, connect);

// * 3. Cmd+Q 快速退出机制

// * 后台线程：等待 400 毫秒保存配置后直接退出进程
static void *force_exit_watchdog(void *arg) {
    (void)arg;
    // ! 预留 400ms 给主线程写回必要的用户偏好与窗口状态，随后直接从内核层退出，杜绝 Dock 栏无响应卡死
    usleep(400000);
    _exit(0);
    return NULL;
}

// * 启动延迟退出机制
static void start_exit_watchdog(void) {
    static dispatch_once_t onceToken;
    dispatch_once(&onceToken, ^{
        // * 触发一次偏好数据同步
        [[NSUserDefaults standardUserDefaults] synchronize];
        pthread_t tid;
        pthread_create(&tid, NULL, force_exit_watchdog, NULL);
        pthread_detach(tid);
    });
}

static void (*orig_terminate)(id, SEL, id);
// * 拦截 AppKit 的 terminate: 消息
static void hooked_terminate(id self, SEL _cmd, id sender) {
    start_exit_watchdog();
    if (orig_terminate) orig_terminate(self, _cmd, sender);
}

// * 拦截标准 exit()：当 Qt 事件循环退出时直接从内核退出，跳过缓慢繁杂的 C++ 静态析构
void my_exit(int status) {
    _exit(status);
}
DYLD_INTERPOSE(my_exit, exit);

// * 4. C++ 内存打补丁核心

// * 根据符号名查找函数地址（兼容带下划线前缀）
static void *resolve_symbol(const char *raw_name) {
    void *addr = dlsym(RTLD_DEFAULT, raw_name);
    if (!addr && raw_name[0] == '_' && raw_name[1] == '_') {
        addr = dlsym(RTLD_DEFAULT, raw_name + 1);
    }
    return addr;
}

// * 将指定符号的函数入口直接覆写为 RET 指令（即刻返回无操作）
static void patch_function_ret(const char *symbol_name) {
    void *addr = resolve_symbol(symbol_name);
    if (!addr) return;

    vm_size_t page_size = (vm_size_t)getpagesize();
    vm_address_t page_start = (vm_address_t)addr & ~(page_size - 1);

    // ! 修改内存页权限为可写，写入完毕后再切回可执行
    if (vm_protect(mach_task_self(), page_start, page_size, FALSE, VM_PROT_READ | VM_PROT_WRITE | VM_PROT_COPY) != KERN_SUCCESS) return;

#if defined(__arm64__) || defined(__aarch64__)
    uint32_t patch = 0xD65F03C0; // * ARM64 指令：RET
    *(uint32_t *)addr = patch;
    sys_icache_invalidate(addr, sizeof(patch));
#elif defined(__x86_64__)
    uint8_t patch = 0xC3; // * x86_64 指令：RET
    *(uint8_t *)addr = patch;
#endif

    vm_protect(mach_task_self(), page_start, page_size, FALSE, VM_PROT_READ | VM_PROT_EXECUTE);
}

// * 将指定符号的函数入口覆写为返回 0 (false) 的指令
static void patch_function_ret_zero(const char *symbol_name) {
    void *addr = resolve_symbol(symbol_name);
    if (!addr) return;

    vm_size_t page_size = (vm_size_t)getpagesize();
    vm_address_t page_start = (vm_address_t)addr & ~(page_size - 1);

    // ! 修改内存页属性
    if (vm_protect(mach_task_self(), page_start, page_size, FALSE, VM_PROT_READ | VM_PROT_WRITE | VM_PROT_COPY) != KERN_SUCCESS) return;

#if defined(__arm64__) || defined(__aarch64__)
    uint32_t *inst = (uint32_t *)addr;
    inst[0] = 0x52800000; // * MOV W0, #0
    inst[1] = 0xD65F03C0; // * RET
    sys_icache_invalidate(addr, 8);
#elif defined(__x86_64__)
    uint8_t *inst = (uint8_t *)addr;
    inst[0] = 0x31; // * XOR EAX, EAX
    inst[1] = 0xC0;
    inst[2] = 0xC3; // * RET
#endif

    vm_protect(mach_task_self(), page_start, page_size, FALSE, VM_PROT_READ | VM_PROT_EXECUTE);
}

// * 5. 动态库加载回调监听

static bool g_f3c_patched = false;
static bool g_apm_patched = false;

// * 当镜像被 dyld 加载时动态打补丁
static void on_image_loaded(const struct mach_header *mh, intptr_t vmaddr_slide) {
    (void)mh;
    (void)vmaddr_slide;
    Dl_info info;
    if (!dladdr(mh, &info) || !info.dli_fname) return;

    // * 1. 屏蔽 F3CReport 埋点上报调度与退出刷新逻辑（防止退出死锁）
    if (!g_f3c_patched && strstr(info.dli_fname, "F3CReport")) {
        g_f3c_patched = true;
        patch_function_ret("__ZN9f3creport14ReportSchedule4InitEv");
        patch_function_ret("__ZN9f3creport14ReportSchedule6UninitEv");
        patch_function_ret("__ZN9f3creport14ReportSchedule5FlushENS_9FlushModeE");
        patch_function_ret("__ZN9f3creport14ReportSchedule13BasicUploader6UploadEv");
        patch_function_ret("__ZN9f3creport14ReportSchedule13BasicUploader9AddRecordERKNSt3__112basic_stringIcNS2_11char_traitsIcEENS2_9allocatorIcEEEE");
        patch_function_ret("__ZN9f3creport14ReportSchedule9AddRecordEiNS_14RecordCategoryERKNSt3__112basic_stringIcNS2_11char_traitsIcEENS2_9allocatorIcEEEE");
        patch_function_ret("__ZN9f3creport9Dump2File4InitEv");
        patch_function_ret("__ZN9f3creport9Dump2File6UninitEv");
        patch_function_ret("__ZN9f3creport9Dump2File10FileHelper5WriteEPKcj");
        patch_function_ret("__ZN9f3creport12SampleEngine4InitEv");
        patch_function_ret("__ZN9f3creport6UnInitEv");
        patch_function_ret("__ZN9f3creport5FlushEv");
    }

    // * 2. 彻底禁用 libFTApmKit.dylib 性能采集与卡顿监控
    if (!g_apm_patched && strstr(info.dli_fname, "libFTApmKit.dylib")) {
        g_apm_patched = true;
        patch_function_ret("__Z17InitCrashReporterv");
        patch_function_ret("__ZN12CFTApmConfig16UpdateFromServerEv");
        patch_function_ret("__ZN12CFTApmConfig19TryUpdateFromServerEv");
        patch_function_ret_zero("__ZN12CFTApmConfig9IsEnabledERK7QStringb");
        patch_function_ret_zero("__ZN12CFTApmConfig20IsCrashReportEnabledEv");
        patch_function_ret_zero("__ZN12CFTApmConfig20IsDesktopJankEnabledEv");
        patch_function_ret_zero("__ZN12CFTApmConfig21IsDesktopErrorEnabledEv");
        patch_function_ret_zero("__ZN12CFTApmConfig22IsMemoryMetricsEnabledEv");
        patch_function_ret_zero("__ZN12CFTApmConfig27IsSceneLatencyReportEnabledEv");
        patch_function_ret_zero("__ZN12CFTApmConfig33IsTransactionLatencyReportEnabledEv");
    }
}

// * 6. 构造函数初始化
__attribute__((constructor))
static void init_patch(void) {
    // ! 检查宿主进程名，确保只对 FTNN 进程注入生效
    const char *prog = getprogname();
    if (!prog || strcmp(prog, "FTNN") != 0) {
        return;
    }

    // * 检查是否已激活补丁环境
    // ? 纯原生 Mach-O 注入时，dyld interpose 需要 DYLD_INSERT_LIBRARIES 激活，这里通过环境变量重新加载一次当前进程
    if (!getenv("FUTU_PATCH_ACTIVE")) {
        setenv("FUTU_PATCH_ACTIVE", "1", 1);
        setenv("DYLD_INSERT_LIBRARIES", "/Applications/Futubull.app/Contents/Frameworks/libfutu_patch.dylib", 1);
        char ***argvp = _NSGetArgv();
        if (argvp && *argvp && (*argvp)[0]) {
            execv("/Applications/Futubull.app/Contents/MacOS/FTNN", *argvp);
        }
    }

    // * 重载完成后清除环境变量，避免污染后续派生的其他子进程
    unsetenv("DYLD_INSERT_LIBRARIES");

    // * 挂载 AppKit 的 terminate: 方法以启动快速退出机制
    Class appCls = objc_getClass("NSApplication");
    if (appCls) {
        Method m = class_getInstanceMethod(appCls, @selector(terminate:));
        if (m) {
            orig_terminate = (void *)method_getImplementation(m);
            method_setImplementation(m, (IMP)hooked_terminate);
        }
    }

    // * 注册镜像加载监听，当动态库加载时自动对内部函数打内存补丁
    _dyld_register_func_for_add_image(on_image_loaded);
}
