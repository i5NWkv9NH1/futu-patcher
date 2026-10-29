// * 补丁安装与客户端处理流程模块

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, error, info, warn};

use crate::cleaner::{
    calculate_dir_size, clean_user_documents, format_size, prune_locales, remove_bloatware,
    resign_app, thin_arm64_binaries,
};
use crate::macho::{InjectResult, inject_file};

// * 针对新版与经典版内嵌的 Objective-C 补丁源码
const FUTUBULL_PATCH_SOURCE: &str = include_str!("../patches/futubull_patch.m");
const FUTUNIUNIU_PATCH_SOURCE: &str = include_str!("../patches/futuniuniu_patch.m");

// * 支持的目标富途客户端枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FutuApp {
    // * Futubull (富途牛牛新版，基于 Qt + CEF，主二进制为 FTNN)
    Futubull,
    // * FutuNiuniu (富途牛牛经典版，Cocoa 原生版，主二进制为 FutuNiuniu)
    FutuNiuniu,
}

impl FutuApp {
    // * 获取 App 安装路径
    pub fn app_path(&self) -> PathBuf {
        match self {
            Self::Futubull => PathBuf::from("/Applications/Futubull.app"),
            Self::FutuNiuniu => PathBuf::from("/Applications/FutuNiuniu.app"),
        }
    }

    // * 获取主二进制文件路径
    pub fn binary_path(&self) -> PathBuf {
        match self {
            Self::Futubull => self.app_path().join("Contents/MacOS/FTNN"),
            Self::FutuNiuniu => self.app_path().join("Contents/MacOS/FutuNiuniu"),
        }
    }

    // * 获取 Frameworks 目录路径
    pub fn frameworks_path(&self) -> PathBuf {
        self.app_path().join("Contents/Frameworks")
    }

    // * 获取对应的 Objective-C 补丁源码字符串
    pub fn patch_source(&self) -> &'static str {
        match self {
            Self::Futubull => FUTUBULL_PATCH_SOURCE,
            Self::FutuNiuniu => FUTUNIUNIU_PATCH_SOURCE,
        }
    }

    // * 客户端显示名称
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Futubull => "Futubull (富途牛牛新版)",
            Self::FutuNiuniu => "FutuNiuniu (富途牛牛经典版)",
        }
    }

    // * 进程名称
    pub fn process_name(&self) -> &'static str {
        match self {
            Self::Futubull => "FTNN",
            Self::FutuNiuniu => "FutuNiuniu",
        }
    }

    // * 检测当前客户端是否已安装在系统上
    pub fn is_installed(&self) -> bool {
        self.app_path().exists()
    }
}

// * 补丁配置参数（支持全选或自由单项组合）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchOptions {
    // * 1. 注入补丁 (隔离文档目录 / 屏蔽埋点请求 / 修复 Cmd+Q 退出卡死)
    pub core_patch: bool,
    // * 2. 删除无用组件 (CrashReporter / NNPython / AutoTest / 虚拟声卡)
    pub remove_bloatware: bool,
    // * 3. 删除多余语言包与 x86 快照
    pub prune_locales: bool,
    // * 4. 提取 arm64 架构 (lipo 移除 x86_64 代码)
    pub thin_arm64: bool,
    // * 5. 清理 ~/Documents 残留文件夹
    pub clean_documents: bool,
}

impl Default for PatchOptions {
    fn default() -> Self {
        Self {
            core_patch: true,
            remove_bloatware: true,
            prune_locales: true,
            thin_arm64: true,
            clean_documents: true,
        }
    }
}

// * 执行汇总统计
#[derive(Debug, Default)]
pub struct PatchReport {
    pub app_name: String,
    pub initial_size: u64,
    pub final_size: u64,
    pub inject_status: Option<InjectResult>,
    pub removed_components: Vec<String>,
    pub pruned_locales: Vec<String>,
    pub thinned_count: usize,
}

// * 检查编译工具链 clang 是否就绪
pub fn check_clang_installed() -> bool {
    Command::new("/usr/bin/clang")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// * 编译内嵌的 Objective-C 补丁为动态库 libfutu_patch.dylib
pub fn compile_patch_dylib(source_code: &str, output_dylib: &Path) -> Result<(), String> {
    let temp_dir = std::env::temp_dir();
    let temp_src = temp_dir.join(format!("futu_patch_{}.m", std::process::id()));

    // * 将源码写出到临时目录
    if let Err(e) = fs::write(&temp_src, source_code) {
        return Err(format!("写入临时补丁源码失败: {e}"));
    }

    debug!("调用 clang 编译动态库: {}", output_dylib.display());

    // * 使用 /usr/bin/clang 进行编译
    let output = Command::new("/usr/bin/clang")
        .arg("-O3")
        .arg("-dynamiclib")
        .arg("-Wall")
        .arg("-Wl,-install_name,@rpath/libfutu_patch.dylib")
        .arg("-framework")
        .arg("Foundation")
        .arg("-framework")
        .arg("AppKit")
        .arg(&temp_src)
        .arg("-o")
        .arg(output_dylib)
        .output();

    // ! 无论成功与否，立即清理临时源码文件
    let _ = fs::remove_file(&temp_src);

    match output {
        Ok(out) => {
            if out.status.success() {
                debug!("动态库编译成功");
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                error!("clang 编译报错:\n{stderr}");
                Err(format!("clang 编译失败: {stderr}"))
            }
        }
        Err(e) => {
            error!("无法启动 /usr/bin/clang: {e}");
            Err(format!(
                "无法执行 /usr/bin/clang，请先安装 Xcode Command Line Tools: {e}"
            ))
        }
    }
}

// * 针对指定富途客户端执行补丁与精简处理
pub fn run_pipeline_for_app(app: FutuApp, options: &PatchOptions) -> Result<PatchReport, String> {
    if !app.is_installed() {
        return Err(format!("未在系统中检测到 {}", app.display_name()));
    }

    let mut report = PatchReport {
        app_name: app.display_name().to_string(),
        ..Default::default()
    };

    let app_path = app.app_path();
    report.initial_size = calculate_dir_size(&app_path);
    info!(
        "处理客户端: 「{}」 (当前体积: {})",
        app.display_name(),
        format_size(report.initial_size)
    );

    // * 1. 终止运行中的客户端进程，避免内核签名缓存冲突
    let proc_name = app.process_name();
    debug!("检查并终止运行中的客户端进程: {proc_name}");
    let kill_res = Command::new("/usr/bin/killall")
        .arg("-9")
        .arg(proc_name)
        .output();
    if let Ok(out) = kill_res
        && out.status.success()
    {
        info!("已终止正在运行的「{proc_name}」进程");
        // * 等待系统释放文件句柄
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    // * 2. 移除无用组件（仅在实际有删除时打印）
    if options.remove_bloatware {
        match remove_bloatware(&app_path) {
            Ok(removed) => {
                if !removed.is_empty() {
                    let sample = removed.join(", ");
                    info!("已删除无用组件: {} 项 ({sample})", removed.len());
                }
                report.removed_components = removed;
            }
            Err(e) => warn!("移除部分组件时遇到错误: {e}"),
        }
    }

    // * 3. 精简 CEF 语言包与快照（仅在实际有删除时打印）
    if options.prune_locales {
        match prune_locales(&app_path) {
            Ok(pruned) => {
                if !pruned.is_empty() {
                    let sample = pruned.join(", ");
                    info!("已删除多余语言包与快照: {} 项 ({sample})", pruned.len());
                }
                report.pruned_locales = pruned;
            }
            Err(e) => warn!("精简语言包时遇到错误: {e}"),
        }
    }

    // * 4. 剥离通用架构，提取纯 arm64 二进制（在注入前完成架构瘦身）
    if options.thin_arm64 {
        match thin_arm64_binaries(&app_path) {
            Ok(count) => {
                if count > 0 {
                    info!("已完成纯 arm64 架构转换: {count} 个通用二进制");
                }
                report.thinned_count = count;
            }
            Err(e) => warn!("提取 arm64 架构时遇到错误: {e}"),
        }
    }

    // * 5. 编译动态库并向纯 arm64 主二进制注入 LC_LOAD_DYLIB
    if options.core_patch {
        let temp_dylib =
            std::env::temp_dir().join(format!("libfutu_patch_{}.dylib", std::process::id()));
        compile_patch_dylib(app.patch_source(), &temp_dylib)?;

        // * 部署动态库到 App 的 Contents/Frameworks/ 目录
        let target_dylib = app.frameworks_path().join("libfutu_patch.dylib");
        if let Err(e) = fs::copy(&temp_dylib, &target_dylib) {
            let _ = fs::remove_file(&temp_dylib);
            return Err(format!(
                "部署动态库到 Frameworks 失败: {e} (请确认是否有管理员权限)"
            ));
        }
        let _ = fs::remove_file(&temp_dylib);

        // * 执行纯 Rust Mach-O 尾部加载表注入
        let main_bin = app.binary_path();
        match inject_file(&main_bin, "@rpath/libfutu_patch.dylib") {
            Ok(result) => {
                report.inject_status = Some(result);
                match result {
                    InjectResult::Injected => {
                        info!("已追加 @rpath/libfutu_patch.dylib 至加载表末尾")
                    }
                    InjectResult::AlreadyPresent => {
                        info!("动态库已在加载表末尾，无需重复写入")
                    }
                    InjectResult::FixedAndInjected => {
                        info!("已修复错位项并重写至加载表末尾")
                    }
                }
            }
            Err(e) => {
                return Err(format!("Mach-O 注入失败: {e}"));
            }
        }
    }

    // * 6. 静默清除隔离属性并执行本地 Ad-hoc 重签名
    if let Err(e) = resign_app(&app_path) {
        warn!("重签名过程中发生警告: {e}");
    }

    report.final_size = calculate_dir_size(&app_path);
    info!(
        "处理完成: 「{}」 (精简后体积: {})",
        app.display_name(),
        format_size(report.final_size)
    );

    Ok(report)
}

// * 执行历史残留文档清理
pub fn run_documents_clean() {
    match clean_user_documents() {
        Ok(paths) => {
            if !paths.is_empty() {
                info!("已清理 ~/Documents 残留缓存: {} 个目录", paths.len());
            }
        }
        Err(e) => warn!("清理 ~/Documents 时发生错误: {e}"),
    }
}
