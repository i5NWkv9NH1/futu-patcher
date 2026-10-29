// * 纯 Rust 实现的 macOS 富途客户端注入动态库 (libfutu_patch.dylib)
// * 替代原 Objective-C 补丁，统一支持 Futubull (新版) 与 FutuNiuniu (经典版)

use std::ffi::{c_char, c_int, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

// * 1. 结构体与常量定义

#[repr(C)]
pub struct InterposeTuple {
    pub replacement: *const (),
    pub replacee: *const (),
}

// ! 允许指针跨线程共享以满足静态生命周期约束
unsafe impl Sync for InterposeTuple {}

#[repr(C)]
pub struct DlInfo {
    pub dli_fname: *const c_char,
    pub dli_fbase: *mut c_void,
    pub dli_sname: *const c_char,
    pub dli_saddr: *mut c_void,
}

pub const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;

// * 2. 系统底层 C / Mach / Objective-C 接口声明

unsafe extern "C" {
    static mach_task_self_: u32;
    fn getpagesize() -> c_int;
    fn sys_icache_invalidate(start: *mut c_void, len: usize);
    fn vm_protect(
        target_task: u32,
        address: usize,
        size: usize,
        set_maximum: i32,
        new_protection: i32,
    ) -> i32;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dladdr(addr: *const c_void, info: *mut DlInfo) -> c_int;
    fn _dyld_register_func_for_add_image(func: unsafe extern "C" fn(*const c_void, isize));

    fn getprogname() -> *const c_char;
    fn getenv(name: *const c_char) -> *mut c_char;
    fn setenv(name: *const c_char, value: *const c_char, overwrite: c_int) -> c_int;
    fn unsetenv(name: *const c_char) -> c_int;
    fn _NSGetArgv() -> *mut *mut *mut c_char;
    fn execv(path: *const c_char, argv: *const *mut c_char) -> c_int;
    fn _exit(status: c_int) -> !;
    fn __error() -> *mut c_int;

    // * 被替换的目标符号
    fn mkdir(path: *const c_char, mode: u16) -> c_int;
    fn access(path: *const c_char, amode: c_int) -> c_int;
    fn stat(path: *const c_char, buf: *mut c_void) -> c_int;
    fn getaddrinfo(
        hostname: *const c_char,
        servname: *const c_char,
        hints: *const c_void,
        res: *mut *mut c_void,
    ) -> c_int;
    fn connect(socket: c_int, address: *const c_void, address_len: u32) -> c_int;
    fn exit(status: c_int) -> !;
    fn NSSearchPathForDirectoriesInDomains(
        directory: usize,
        domain_mask: usize,
        expand_tilde: i8,
    ) -> *mut c_void;

    // * Objective-C Runtime 接口
    fn objc_getClass(name: *const c_char) -> *mut c_void;
    fn sel_registerName(name: *const c_char) -> *mut c_void;
    fn class_getInstanceMethod(cls: *mut c_void, name: *mut c_void) -> *mut c_void;
    fn class_getClassMethod(cls: *mut c_void, name: *mut c_void) -> *mut c_void;
    fn method_getImplementation(method: *mut c_void) -> *mut c_void;
    fn method_setImplementation(method: *mut c_void, imp: *mut c_void) -> *mut c_void;
}

// * 3. 路径隔离与沙盒重定向

static ISOLATED_DOCS_PATH: OnceLock<Vec<u8>> = OnceLock::new();
static ISOLATED_DOCS_ARRAY: OnceLock<usize> = OnceLock::new();

// * 获取沙盒隔离目录，并将腾讯 IMSDK 与本地化冗余数据移出 ~/Documents
fn get_isolated_documents_path() -> &'static [u8] {
    ISOLATED_DOCS_PATH.get_or_init(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/Shared".to_string());
        let prog = unsafe { getprogname() };
        let is_futuniuniu = if !prog.is_null() {
            let s = unsafe { std::ffi::CStr::from_ptr(prog) };
            s.to_bytes() == b"FutuNiuniu"
        } else {
            false
        };

        let app_folder = if is_futuniuniu {
            "FutuNiuniu"
        } else {
            "Futubull"
        };
        let path = format!("{home}/Library/Application Support/{app_folder}/SandboxedDocuments");

        // * 递归创建隔离目录
        let _ = std::fs::create_dir_all(&path);

        let mut bytes = path.into_bytes();
        bytes.push(0);
        bytes
    })
}

// * 构建供 NSSearchPathForDirectoriesInDomains 返回的常驻 NSArray 对象
fn get_isolated_documents_array() -> *mut c_void {
    let ptr_val = *ISOLATED_DOCS_ARRAY.get_or_init(|| {
        let iso_bytes = get_isolated_documents_path();
        unsafe {
            let str_cls = objc_getClass(c"NSString".as_ptr());
            let sel_str = sel_registerName(c"stringWithUTF8String:".as_ptr());
            let msg_send: *mut c_void = dlsym(RTLD_DEFAULT, c"objc_msgSend".as_ptr());
            let func_str: unsafe extern "C" fn(
                *mut c_void,
                *mut c_void,
                *const c_char,
            ) -> *mut c_void = std::mem::transmute(msg_send);
            let ns_str = func_str(str_cls, sel_str, iso_bytes.as_ptr() as *const c_char);

            let arr_cls = objc_getClass(c"NSArray".as_ptr());
            let sel_arr = sel_registerName(c"arrayWithObject:".as_ptr());
            let func_arr: unsafe extern "C" fn(
                *mut c_void,
                *mut c_void,
                *mut c_void,
            ) -> *mut c_void = std::mem::transmute(msg_send);
            let ns_arr = func_arr(arr_cls, sel_arr, ns_str);

            // * 调用 CFRetain 锁定对象生命周期，防止被自动释放池销毁
            let cf_retain: *mut c_void = dlsym(RTLD_DEFAULT, c"CFRetain".as_ptr());
            if !cf_retain.is_null() {
                let retain_fn: unsafe extern "C" fn(*mut c_void) -> *mut c_void =
                    std::mem::transmute(cf_retain);
                retain_fn(ns_arr);
            }
            ns_arr as usize
        }
    });
    ptr_val as *mut c_void
}

// * 检查并自动重写含有 com_tencent_imsdk_data 或 LocalizedString 的路径
unsafe fn redirect_sandbox_path(path: *const c_char, buf: &mut [u8]) -> *const c_char {
    if path.is_null() {
        return path;
    }
    let cstr = unsafe { std::ffi::CStr::from_ptr(path) };
    let bytes = cstr.to_bytes();

    // ! 同时匹配腾讯 IMSDK 缓存以及 LocalizedString 残留文件夹
    let pos = bytes
        .windows(b"com_tencent_imsdk_data".len())
        .position(|w| w == b"com_tencent_imsdk_data")
        .or_else(|| {
            bytes
                .windows(b"LocalizedString".len())
                .position(|w| w == b"LocalizedString")
        });

    if let Some(idx) = pos {
        let sub = &bytes[idx..];
        let iso = get_isolated_documents_path();
        let iso_prefix = &iso[..iso.len() - 1]; // 去除结尾的 \0
        let total_len = iso_prefix.len() + 1 + sub.len() + 1;
        if total_len <= buf.len() {
            buf[..iso_prefix.len()].copy_from_slice(iso_prefix);
            buf[iso_prefix.len()] = b'/';
            let sub_start = iso_prefix.len() + 1;
            buf[sub_start..sub_start + sub.len()].copy_from_slice(sub);
            buf[sub_start + sub.len()] = 0;
            return buf.as_ptr() as *const c_char;
        }
    }
    path
}

// * 4. 符号替换实现 (Interpose Handlers)

#[unsafe(no_mangle)]
pub unsafe extern "C" fn my_mkdir(path: *const c_char, mode: u16) -> c_int {
    let mut buf = [0u8; 1024];
    let real_path = unsafe { redirect_sandbox_path(path, &mut buf) };
    unsafe { mkdir(real_path, mode) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn my_access(path: *const c_char, amode: c_int) -> c_int {
    let mut buf = [0u8; 1024];
    let real_path = unsafe { redirect_sandbox_path(path, &mut buf) };
    unsafe { access(real_path, amode) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn my_stat(path: *const c_char, buf: *mut c_void) -> c_int {
    let mut local_buf = [0u8; 1024];
    let real_path = unsafe { redirect_sandbox_path(path, &mut local_buf) };
    unsafe { stat(real_path, buf) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn my_getaddrinfo(
    hostname: *const c_char,
    servname: *const c_char,
    hints: *const c_void,
    res: *mut *mut c_void,
) -> c_int {
    if !hostname.is_null() {
        let name_bytes = unsafe { std::ffi::CStr::from_ptr(hostname) }.to_bytes();
        // ! 拦截各类数据采集与崩溃上报域名，直接返回找不到主机名 (EAI_NONAME = 8)
        if name_bytes.windows(8).any(|w| w == b"collect.")
            || name_bytes.windows(8).any(|w| w == b"apm.futu")
            || name_bytes.windows(11).any(|w| w == b"crashlytics")
            || name_bytes.windows(8).any(|w| w == b"firebase")
            || name_bytes.windows(14).any(|w| w == b"googleapis.com")
            || name_bytes.windows(15).any(|w| w == b"app-measurement")
        {
            return 8;
        }
    }
    unsafe { getaddrinfo(hostname, servname, hints, res) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn my_connect(
    socket: c_int,
    address: *const c_void,
    address_len: u32,
) -> c_int {
    if !address.is_null() && address_len >= 8 {
        let bytes = unsafe { std::slice::from_raw_parts(address as *const u8, address_len as usize) };
        // ! sa_family 位于偏移 1 (AF_INET = 2)
        if bytes[1] == 2 {
            // ! 读取 4..8 偏移处的 32 位 IPv4 地址
            let ip = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
            // ! 匹配富途已知的两组直连数据上报 IP: 42.193.128.62 与 129.226.79.246
            if ip == 0x2AC1803E || ip == 0x81E24FF6 {
                let err_ptr = unsafe { __error() };
                if !err_ptr.is_null() {
                    unsafe { *err_ptr = 61 }; // ECONNREFUSED = 61
                }
                return -1;
            }
        }
    }
    unsafe { connect(socket, address, address_len) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn my_exit(status: c_int) -> ! {
    // ! Qt 事件循环退出时直接从内核退出，跳过缓慢繁杂的 C++ 静态析构
    unsafe { _exit(status) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn my_NSSearchPathForDirectoriesInDomains(
    directory: usize,
    domain_mask: usize,
    expand_tilde: i8,
) -> *mut c_void {
    // ! 9 代表 NSDocumentDirectory
    if directory == 9 {
        return get_isolated_documents_array();
    }
    unsafe { NSSearchPathForDirectoriesInDomains(directory, domain_mask, expand_tilde) }
}

// * 注册 dyld 加载表符号替换清单
#[used]
#[unsafe(link_section = "__DATA,__interpose")]
pub static INTERPOSES: [InterposeTuple; 7] = [
    InterposeTuple {
        replacement: my_mkdir as *const (),
        replacee: mkdir as *const (),
    },
    InterposeTuple {
        replacement: my_access as *const (),
        replacee: access as *const (),
    },
    InterposeTuple {
        replacement: my_stat as *const (),
        replacee: stat as *const (),
    },
    InterposeTuple {
        replacement: my_getaddrinfo as *const (),
        replacee: getaddrinfo as *const (),
    },
    InterposeTuple {
        replacement: my_connect as *const (),
        replacee: connect as *const (),
    },
    InterposeTuple {
        replacement: my_exit as *const (),
        replacee: exit as *const (),
    },
    InterposeTuple {
        replacement: my_NSSearchPathForDirectoriesInDomains as *const (),
        replacee: NSSearchPathForDirectoriesInDomains as *const (),
    },
];

// * 5. Cmd+Q 快速退出机制

static mut ORIG_TERMINATE: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void)> =
    None;
static WATCHDOG_ONCE: std::sync::Once = std::sync::Once::new();

// * 启动延迟退出机制：预留 400ms 写回配置后直接从内核退出
fn start_exit_watchdog() {
    WATCHDOG_ONCE.call_once(|| {
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(400));
            unsafe { _exit(0) };
        });
    });
}

unsafe extern "C" fn hooked_terminate(this: *mut c_void, sel: *mut c_void, sender: *mut c_void) {
    start_exit_watchdog();
    if let Some(orig) = unsafe { ORIG_TERMINATE } {
        unsafe { orig(this, sel, sender) };
    }
}

// * 挂载 AppKit 的 terminate: 消息拦截
fn hook_appkit_terminate() {
    unsafe {
        let app_cls = objc_getClass(c"NSApplication".as_ptr());
        if app_cls.is_null() {
            return;
        }
        let sel = sel_registerName(c"terminate:".as_ptr());
        let method = class_getInstanceMethod(app_cls, sel);
        if !method.is_null() {
            let orig = method_getImplementation(method);
            ORIG_TERMINATE = Some(std::mem::transmute(orig));
            method_setImplementation(method, hooked_terminate as *mut c_void);
        }
    }
}

// * 6. 内存打补丁核心 (RET / RET_ZERO 覆写)

unsafe fn resolve_symbol(name: *const c_char) -> *mut c_void {
    let mut addr = unsafe { dlsym(RTLD_DEFAULT, name) };
    if addr.is_null() {
        let bytes = unsafe { std::ffi::CStr::from_ptr(name) }.to_bytes();
        if bytes.len() > 2 && bytes[0] == b'_' && bytes[1] == b'_' {
            addr = unsafe { dlsym(RTLD_DEFAULT, name.add(1)) };
        }
    }
    addr
}

unsafe fn patch_function_ret(name: *const c_char) {
    let addr = unsafe { resolve_symbol(name) };
    if addr.is_null() {
        return;
    }
    let page_size = unsafe { getpagesize() } as usize;
    let page_start = (addr as usize) & !(page_size - 1);

    // ! VM_PROT_READ | VM_PROT_WRITE | VM_PROT_COPY = 0x13
    if unsafe { vm_protect(mach_task_self_, page_start, page_size, 0, 0x13) } != 0 {
        return;
    }

    #[cfg(target_arch = "aarch64")]
    {
        let p = addr as *mut u32;
        unsafe {
            *p = 0xD65F03C0; // * ARM64 指令：RET
            sys_icache_invalidate(addr, 4);
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        let p = addr as *mut u8;
        unsafe {
            *p = 0xC3; // * x86_64 指令：RET
        }
    }

    // ! VM_PROT_READ | VM_PROT_EXECUTE = 0x05
    unsafe { vm_protect(mach_task_self_, page_start, page_size, 0, 0x05) };
}

unsafe fn patch_function_ret_zero(name: *const c_char) {
    let addr = unsafe { resolve_symbol(name) };
    if addr.is_null() {
        return;
    }
    let page_size = unsafe { getpagesize() } as usize;
    let page_start = (addr as usize) & !(page_size - 1);

    if unsafe { vm_protect(mach_task_self_, page_start, page_size, 0, 0x13) } != 0 {
        return;
    }

    #[cfg(target_arch = "aarch64")]
    {
        let p = addr as *mut u32;
        unsafe {
            *p = 0x52800000; // * ARM64: MOV W0, #0
            *p.add(1) = 0xD65F03C0; // * ARM64: RET
            sys_icache_invalidate(addr, 8);
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        let p = addr as *mut u8;
        unsafe {
            *p = 0x31; // * x86_64: XOR EAX, EAX
            *p.add(1) = 0xC0;
            *p.add(2) = 0xC3; // * x86_64: RET
        }
    }

    unsafe { vm_protect(mach_task_self_, page_start, page_size, 0, 0x05) };
}

// * 7. 经典版 FutuNiuniu 专属 Firebase 与 GDTCOR 禁用

unsafe extern "C" fn dummy_empty(_this: *mut c_void, _cmd: *mut c_void) {}
unsafe extern "C" fn dummy_nil(_this: *mut c_void, _cmd: *mut c_void) -> *mut c_void {
    std::ptr::null_mut()
}

unsafe fn disable_objc_class_method(
    class_name: *const c_char,
    sel_name: *const c_char,
    new_imp: *mut c_void,
) {
    let cls = unsafe { objc_getClass(class_name) };
    if cls.is_null() {
        return;
    }
    let sel = unsafe { sel_registerName(sel_name) };
    let m = unsafe { class_getClassMethod(cls, sel) };
    if !m.is_null() {
        unsafe { method_setImplementation(m, new_imp) };
    }
}

fn disable_classic_analytics() {
    unsafe {
        disable_objc_class_method(
            c"FIRApp".as_ptr(),
            c"configure".as_ptr(),
            dummy_empty as *mut c_void,
        );
        disable_objc_class_method(
            c"FIRApp".as_ptr(),
            c"configureWithOptions:".as_ptr(),
            dummy_empty as *mut c_void,
        );
        disable_objc_class_method(
            c"FIRApp".as_ptr(),
            c"defaultApp".as_ptr(),
            dummy_nil as *mut c_void,
        );
        disable_objc_class_method(
            c"FIRCrashlytics".as_ptr(),
            c"crashlytics".as_ptr(),
            dummy_nil as *mut c_void,
        );
        disable_objc_class_method(
            c"GDTCORUploadCoordinator".as_ptr(),
            c"sharedInstance".as_ptr(),
            dummy_nil as *mut c_void,
        );
        disable_objc_class_method(
            c"GDTCORRegistrar".as_ptr(),
            c"sharedInstance".as_ptr(),
            dummy_nil as *mut c_void,
        );
    }
}

// * 8. 模块函数拦截清单

fn patch_f3c_report() {
    unsafe {
        patch_function_ret(c"__ZN9f3creport4InitEv".as_ptr());
        patch_function_ret(c"__ZN9f3creport15IFTClientReport4InitEv".as_ptr());
        patch_function_ret(c"__ZN9f3creport14ReportSchedule4InitEv".as_ptr());
        patch_function_ret(c"__ZN9f3creport14ReportSchedule6UninitEv".as_ptr());
        patch_function_ret(c"__ZN9f3creport14ReportSchedule5FlushENS_9FlushModeE".as_ptr());
        patch_function_ret(c"__ZN9f3creport14ReportSchedule13BasicUploader6UploadEv".as_ptr());
        patch_function_ret(
            c"__ZN9f3creport14ReportSchedule13BasicUploader9AddRecordERKNSt3__112basic_stringIcNS2_11char_traitsIcEENS2_9allocatorIcEEEE"
                .as_ptr(),
        );
        patch_function_ret(
            c"__ZN9f3creport14ReportSchedule9AddRecordEiNS_14RecordCategoryERKNSt3__112basic_stringIcNS2_11char_traitsIcEENS2_9allocatorIcEEEE"
                .as_ptr(),
        );
        patch_function_ret(
            c"__ZN9f3creport15FAPMReportAgent8DoReportERKNSt3__112basic_stringIcNS1_11char_traitsIcEENS1_9allocatorIcEEEES9_S9_S9_b"
                .as_ptr(),
        );
        patch_function_ret(
            c"__ZN9f3creport20FTNetReportAgentImpl10ReportFAPMERKNSt3__112basic_stringIcNS1_11char_traitsIcEENS1_9allocatorIcEEEES9_S9_S9_b"
                .as_ptr(),
        );
        patch_function_ret(
            c"__ZN9f3creport20FTNetReportAgentImpl11ReportTrackERKNSt3__112basic_stringIcNS1_11char_traitsIcEENS1_9allocatorIcEEEEiS9_ib"
                .as_ptr(),
        );
        patch_function_ret(
            c"__ZN9f3creport20FTNetReportAgentImpl12ReportFTraceERKN6f3cnet10FTraceInfoE"
                .as_ptr(),
        );
        patch_function_ret(
            c"__ZN9f3creport21AddEventMonitorReportERKNS_22EventMonitorReportDataE".as_ptr(),
        );
        patch_function_ret(c"__ZN9f3creport9Dump2File4InitEv".as_ptr());
        patch_function_ret(c"__ZN9f3creport9Dump2File6UninitEv".as_ptr());
        patch_function_ret(c"__ZN9f3creport9Dump2File10FileHelper5WriteEPKcj".as_ptr());
        patch_function_ret(c"__ZN9f3creport12SampleEngine4InitEv".as_ptr());
        patch_function_ret(c"__ZN9f3creport6UnInitEv".as_ptr());
        patch_function_ret(c"__ZN9f3creport5FlushEv".as_ptr());
    }
}

fn patch_ft_apm() {
    unsafe {
        patch_function_ret(c"__ZN5ftapm4InitEv".as_ptr());
        patch_function_ret(c"__ZN5ftapm14InitAfterLoginEv".as_ptr());
        patch_function_ret(c"__ZN13IFTApmManager4InitEv".as_ptr());
        patch_function_ret(c"__ZN13IFTApmManager15StartJankDetectEv".as_ptr());
        patch_function_ret(c"__ZN13CFTApmManager13InitApmConfigEv".as_ptr());
        patch_function_ret(c"__ZN13CFTApmManager15StartJankDetectEv".as_ptr());
        patch_function_ret(c"__ZN13CFTApmManager18StartMemoryMonitorEv".as_ptr());
        patch_function_ret(c"__ZN13CFTApmManager28AsyncInitApmConfigAfterLoginEv".as_ptr());
        patch_function_ret(c"__ZN16FTUiJankDetector5StartERK18UiJankDetectOption".as_ptr());
        patch_function_ret(c"__ZN18UiJankDetectThread5StartEv".as_ptr());
        patch_function_ret(c"__ZN19CFTApmMemoryMonitor5StartERK19MemoryMonitorOption".as_ptr());
        patch_function_ret(c"__ZN24CFTApmPerformanceMonitor5StartERK18UiJankDetectOption".as_ptr());
        patch_function_ret(c"__ZN25CFTApmMemoryMonitorWorker5StartERK19MemoryMonitorOption".as_ptr());
        patch_function_ret(c"__ZN16CFTCrashReporter4InitERK19CrashReporterConfig".as_ptr());
        patch_function_ret(c"__Z17InitCrashReporterv".as_ptr());
        patch_function_ret(c"__ZN12CFTApmConfig16UpdateFromServerEv".as_ptr());
        patch_function_ret(c"__ZN12CFTApmConfig19TryUpdateFromServerEv".as_ptr());
        patch_function_ret_zero(c"__ZN12CFTApmConfig9IsEnabledERK7QStringb".as_ptr());
        patch_function_ret_zero(c"__ZN12CFTApmConfig20IsCrashReportEnabledEv".as_ptr());
        patch_function_ret_zero(c"__ZN12CFTApmConfig20IsDesktopJankEnabledEv".as_ptr());
        patch_function_ret_zero(c"__ZN12CFTApmConfig21IsDesktopErrorEnabledEv".as_ptr());
        patch_function_ret_zero(c"__ZN12CFTApmConfig22IsMemoryMetricsEnabledEv".as_ptr());
        patch_function_ret_zero(c"__ZN12CFTApmConfig27IsSceneLatencyReportEnabledEv".as_ptr());
        patch_function_ret_zero(c"__ZN12CFTApmConfig33IsTransactionLatencyReportEnabledEv".as_ptr());
        patch_function_ret_zero(
            c"__ZN8crashpad14CrashpadClient12StartHandlerERKN4base8FilePathES4_S4_RKNSt3__112basic_stringIcNS5_11char_traitsIcEENS5_9allocatorIcEEEESD_RKNS5_3mapISB_SB_NS5_4lessISB_EENS9_INS5_4pairISC_SB_EEEEEERKNS5_6vectorISB_NS9_ISB_EEEEbbRKNSN_IS2_NS9_IS2_EEEE"
                .as_ptr(),
        );
        patch_function_ret(c"__ZN8crashpad8Settings17SetUploadsEnabledEb".as_ptr());
        patch_function_ret(c"__ZN18FTApmErrorReporter10SetEnabledEb".as_ptr());
        patch_function_ret(
            c"__ZN24FTApmTimeMetricsReporter11ReportSceneERKN4core3apm5SceneE".as_ptr(),
        );
        patch_function_ret(
            c"__ZN24FTApmTimeMetricsReporter17ReportTransactionERKN4core3apm11TransactionE".as_ptr(),
        );
        patch_function_ret(
            c"__ZN4core3apm14error_reporter6ReportERKNS1_9ErrorDataE14ApmReportLevelb".as_ptr(),
        );
    }
}

fn patch_log_uploader() {
    unsafe {
        patch_function_ret(c"__ZN15f3clog_uploader4InitEv".as_ptr());
        patch_function_ret(c"__ZN15f3clog_uploader11TaskManager4InitEv".as_ptr());
        patch_function_ret(c"__ZN15f3clog_uploader13UploadLogTask5StartEv".as_ptr());
        patch_function_ret(c"__ZN15f3clog_uploader8Uploader10UploadFileEv".as_ptr());
        patch_function_ret(
            c"__ZN15f3clog_uploader18StartLogUploadTaskERKNS_8TaskInfoERKNSt3__16vectorINS_9FTLogInfoENS3_9allocatorIS5_EEEERKNS4_INS_13CustomLogItemENS6_ISB_EEEENS3_8functionIFvjNS_9ErrorCodeEPKNS4_INS3_12basic_stringIcNS3_11char_traitsIcEENS6_IcEEEENS6_ISM_EEEEEEE"
                .as_ptr(),
        );
    }
}

// * 9. 动态库加载监听与构造函数初始化

static G_F3C_PATCHED: AtomicBool = AtomicBool::new(false);
static G_APM_PATCHED: AtomicBool = AtomicBool::new(false);
static G_LOG_UPLOADER_PATCHED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn on_image_loaded(header: *const c_void, _slide: isize) {
    let mut info = std::mem::MaybeUninit::<DlInfo>::uninit();
    if unsafe { dladdr(header, info.as_mut_ptr()) } == 0 {
        return;
    }
    let info = unsafe { info.assume_init() };
    if info.dli_fname.is_null() {
        return;
    }
    let fname = unsafe { std::ffi::CStr::from_ptr(info.dli_fname) }.to_bytes();

    if !G_F3C_PATCHED.load(Ordering::Relaxed) && fname.windows(9).any(|w| w == b"F3CReport") {
        G_F3C_PATCHED.store(true, Ordering::Relaxed);
        patch_f3c_report();
    }

    if !G_APM_PATCHED.load(Ordering::Relaxed)
        && fname.windows(17).any(|w| w == b"libFTApmKit.dylib")
    {
        G_APM_PATCHED.store(true, Ordering::Relaxed);
        patch_ft_apm();
    }

    if !G_LOG_UPLOADER_PATCHED.load(Ordering::Relaxed)
        && fname.windows(14).any(|w| w == b"F3CLogUploader")
    {
        G_LOG_UPLOADER_PATCHED.store(true, Ordering::Relaxed);
        patch_log_uploader();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn init_patch() {
    let prog = unsafe { getprogname() };
    if prog.is_null() {
        return;
    }
    let prog_cstr = unsafe { std::ffi::CStr::from_ptr(prog) };
    let prog_bytes = prog_cstr.to_bytes();

    let is_ftnn = prog_bytes == b"FTNN";
    let is_futuniuniu = prog_bytes == b"FutuNiuniu";

    // ! 只针对 FTNN 与 FutuNiuniu 宿主生效
    if !is_ftnn && !is_futuniuniu {
        return;
    }

    let (exec_path, dylib_path) = if is_ftnn {
        (
            c"/Applications/Futubull.app/Contents/MacOS/FTNN",
            c"/Applications/Futubull.app/Contents/Frameworks/libfutu_patch.dylib",
        )
    } else {
        (
            c"/Applications/FutuNiuniu.app/Contents/MacOS/FutuNiuniu",
            c"/Applications/FutuNiuniu.app/Contents/Frameworks/libfutu_patch.dylib",
        )
    };

    // * 检查是否已激活补丁环境
    // ? 纯原生 Mach-O 注入时，dyld interpose 需要 DYLD_INSERT_LIBRARIES 激活，这里通过环境变量重新加载一次当前进程
    let active = unsafe { getenv(c"FUTU_PATCH_ACTIVE".as_ptr()) };
    if active.is_null() {
        unsafe {
            setenv(c"FUTU_PATCH_ACTIVE".as_ptr(), c"1".as_ptr(), 1);
            setenv(c"DYLD_INSERT_LIBRARIES".as_ptr(), dylib_path.as_ptr(), 1);
            let argv_ptr = _NSGetArgv();
            if !argv_ptr.is_null() && !(*argv_ptr).is_null() {
                execv(exec_path.as_ptr(), *argv_ptr as *const *mut c_char);
            }
        }
    }

    // * 重载完成后清除环境变量，避免污染后续派生的其他子进程
    unsafe {
        unsetenv(c"DYLD_INSERT_LIBRARIES".as_ptr());
    }

    // * 挂载 AppKit 的 terminate: 方法以启动快速退出机制
    hook_appkit_terminate();

    // * 如果是经典版 FutuNiuniu，禁用 Firebase 与 GDTCOR 上报
    if is_futuniuniu {
        disable_classic_analytics();
    }

    // * 注册镜像加载监听，当动态库加载时自动对内部函数打内存补丁
    unsafe {
        _dyld_register_func_for_add_image(on_image_loaded);
    }
}

// * 注册 Mach-O 构造函数入口
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
pub static INIT_FUNC: unsafe extern "C" fn() = init_patch;
