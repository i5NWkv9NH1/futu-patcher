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
#include <sys/stat.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <netdb.h>
#include <mach/mach.h>
#include <mach/vm_map.h>
#include <libkern/OSCacheControl.h>

// * 1. 路径隔离与 POSIX 底层重定向

// * 获取沙盒隔离目录，将腾讯 IMSDK 与本地化冗余垃圾移出 ~/Documents
static NSString *GetIsolatedDocumentsPath(void) {
    static NSString *isolatedPath = nil;
    static dispatch_once_t onceToken;
    dispatch_once(&onceToken, ^{
        NSString *home = NSHomeDirectory();
        isolatedPath = [home stringByAppendingPathComponent:@"Library/Application Support/FutuNiuniu/SandboxedDocuments"];
        // * 如果目录不存在则自动递归创建
        [[NSFileManager defaultManager] createDirectoryAtPath:isolatedPath withIntermediateDirectories:YES attributes:nil error:nil];
    });
    return isolatedPath;
}

// * 检查并自动重定向含有 com_tencent_imsdk_data 或 LocalizedString 的垃圾路径
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

// * 拦截 mkdir
int my_mkdir(const char *path, mode_t mode) {
    char buf[1024];
    return mkdir(redirect_sandbox_path(path, buf, sizeof(buf)), mode);
}
DYLD_INTERPOSE(my_mkdir, mkdir);

// * 拦截 open
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

// * 拦截 access
int my_access(const char *path, int amode) {
    char buf[1024];
    return access(redirect_sandbox_path(path, buf, sizeof(buf)), amode);
}
DYLD_INTERPOSE(my_access, access);

// * 拦截 stat
int my_stat(const char *path, struct stat *buf_stat) {
    char buf[1024];
    return stat(redirect_sandbox_path(path, buf, sizeof(buf)), buf_stat);
}
DYLD_INTERPOSE(my_stat, stat);

// * 拦截 Cocoa 的 NSSearchPathForDirectoriesInDomains
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

// * 3. Cmd+Q 秒退守护机制

// * 强制退出看门狗后台线程
static void *force_exit_watchdog(void *arg) {
    (void)arg;
    // ! 预留 400ms 给主线程写回必要的用户偏好与窗口状态，随后直接从内核层退出，杜绝 Dock 栏无响应卡死
    usleep(400000);
    _exit(0);
    return NULL;
}

// * 启动退出看门狗
static void start_exit_watchdog(void) {
    static dispatch_once_t onceToken;
    dispatch_once(&onceToken, ^{
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

// * 拦截标准 exit()：直接从内核退出，跳过缓慢析构
void my_exit(int status) {
    _exit(status);
}
DYLD_INTERPOSE(my_exit, exit);

// * 4. C++ 内存 RET 打补丁

// * 根据符号名查找函数地址
static void *resolve_symbol(void *handle, const char *raw_name) {
    void *addr = dlsym(handle, raw_name);
    if (addr) return addr;
    if (raw_name[0] == '_' && raw_name[1] == '_') {
        addr = dlsym(handle, raw_name + 1);
        if (addr) return addr;
    }
    if (handle != RTLD_DEFAULT) {
        addr = dlsym(RTLD_DEFAULT, raw_name);
        if (!addr && raw_name[0] == '_' && raw_name[1] == '_') {
            addr = dlsym(RTLD_DEFAULT, raw_name + 1);
        }
    }
    return addr;
}

// * 将指定符号的函数入口直接覆写为 RET 指令
static void patch_function_ret(void *handle, const char *symbol_name) {
    void *addr = resolve_symbol(handle, symbol_name);
    if (!addr) return;

    vm_size_t page_size = (vm_size_t)getpagesize();
    vm_address_t page_start = (vm_address_t)addr & ~(page_size - 1);

    // ! 修改内存页权限为可写
    kern_return_t kr = vm_protect(mach_task_self(), page_start, page_size, FALSE,
                                  VM_PROT_READ | VM_PROT_WRITE | VM_PROT_COPY);
    if (kr != KERN_SUCCESS) return;

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

// * 5. 禁用 Firebase / GoogleDataTransport

static void Dummy_Empty(id self, SEL _cmd) {
    (void)self;
    (void)_cmd;
}

static id Dummy_Nil(id self, SEL _cmd) {
    (void)self;
    (void)_cmd;
    return nil;
}

// * 替换 Objective-C 类方法为空实现
static void disable_objc_class_method(const char *className, const char *selName, IMP newImp) {
    Class cls = objc_getClass(className);
    if (!cls) return;
    SEL sel = sel_registerName(selName);
    Method m = class_getClassMethod(cls, sel);
    if (m) {
        method_setImplementation(m, newImp);
    }
}

// * 6. 构造函数初始化
__attribute__((constructor))
static void init_patch(void) {
    // ! 检查宿主进程名，确保只对 FutuNiuniu 进程生效
    const char *prog = getprogname();
    if (!prog || strcmp(prog, "FutuNiuniu") != 0) {
        return;
    }

    // * 检查是否已经处于自举子进程环境
    // ? 纯原生 Mach-O 注入时，dyld interpose 需要 DYLD_INSERT_LIBRARIES 激活，这里自举重启一次
    if (!getenv("FUTU_PATCH_ACTIVE")) {
        setenv("FUTU_PATCH_ACTIVE", "1", 1);
        setenv("DYLD_INSERT_LIBRARIES", "/Applications/FutuNiuniu.app/Contents/Frameworks/libfutu_patch.dylib", 1);
        char ***argvp = _NSGetArgv();
        if (argvp && *argvp && (*argvp)[0]) {
            execv("/Applications/FutuNiuniu.app/Contents/MacOS/FutuNiuniu", *argvp);
        }
    }

    // * 自举完成后清除环境变量，避免污染后续派生的其他子进程
    unsetenv("DYLD_INSERT_LIBRARIES");

    // * 挂载 AppKit 的 terminate: 方法以启动退出看门狗
    Class appCls = objc_getClass("NSApplication");
    if (appCls) {
        Method m = class_getInstanceMethod(appCls, @selector(terminate:));
        if (m) {
            orig_terminate = (void *)method_getImplementation(m);
            method_setImplementation(m, (IMP)hooked_terminate);
        }
    }

    // * 1. 禁用 Firebase & GoogleDataTransport 上报
    disable_objc_class_method("FIRApp", "configure", (IMP)Dummy_Empty);
    disable_objc_class_method("FIRApp", "configureWithOptions:", (IMP)Dummy_Empty);
    disable_objc_class_method("FIRApp", "defaultApp", (IMP)Dummy_Nil);
    disable_objc_class_method("FIRCrashlytics", "crashlytics", (IMP)Dummy_Nil);
    disable_objc_class_method("GDTCORUploadCoordinator", "sharedInstance", (IMP)Dummy_Nil);
    disable_objc_class_method("GDTCORRegistrar", "sharedInstance", (IMP)Dummy_Nil);

    // * 2. 掐死 F3CReport C++ 核心
    const char *framework_path = "/Applications/FutuNiuniu.app/Contents/Frameworks/F3CReport.framework/Versions/Current/F3CReport";
    void *f3c_handle = dlopen(framework_path, RTLD_NOW | RTLD_GLOBAL);
    if (!f3c_handle) f3c_handle = RTLD_DEFAULT;

    patch_function_ret(f3c_handle, "__ZN9f3creport4InitEv");
    patch_function_ret(f3c_handle, "__ZN9f3creport14ReportSchedule4InitEv");
    patch_function_ret(f3c_handle, "__ZN9f3creport9Dump2File10FileHelper5WriteEPKcj");
}
