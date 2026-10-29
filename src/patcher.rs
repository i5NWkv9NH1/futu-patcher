// * 核心补丁流水线与客户端治理模块

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, error, info, warn};

use crate::cleaner::{
    calculate_dir_size, clean_user_documents, format_size, prune_locales, remove_bloatware,
    resign_app, thin_arm64_binaries,
};
use crate::macho::{InjectResult, inject_file};

// * 针对新版与旧版内嵌的 Objective-C 补丁源码
const FUTUBULL_PATCH_SOURCE: &str = include_str!("../patches/futubull_patch.m");
const FUTUNIUNIU_PATCH_SOURCE: &str = include_str!("../patches/futuniuniu_patch.m");

// * 支持的目标富途客户端枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FutuApp {
    // * 新版 Futubull (基于 Qt + CEF，主二进制为 FTNN)
    Futubull,
    // * 旧版 FutuNiuniu (Cocoa 原生版，主二进制为 FutuNiuniu)
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

    // * 获取可能残留的历史 .real 备份文件路径
    pub fn real_binary_path(&self) -> PathBuf {
        match self {
            Self::Futubull => self.app_path().join("Contents/MacOS/FTNN.real"),
            Self::FutuNiuniu => self.app_path().join("Contents/MacOS/FutuNiuniu.real"),
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
            Self::Futubull => "Futubull (新版长期维护版)",
            Self::FutuNiuniu => "FutuNiuniu (旧版 legacy 原生版)",
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

// * 补丁流水线配置参数（支持用户全选或自由单项组合）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchOptions {
    // * 1. 核心注入补丁 (IMSDK 沙盒隔离 + 网络双重网闸 + 内存防上报 + Cmd+Q 秒退看门狗)
    pub core_patch: bool,
    // * 2. 移除冗余组件 (CrashReporter、NNPython、AutoTest 等)
    pub remove_bloatware: bool,
    // * 3. 精简语言包与快照 (CEF 非必要语言包 + x86_64 快照)
    pub prune_locales: bool,
    // * 4. 提取纯 arm64 架构 (lipo 剥离 x86_64)
    pub thin_arm64: bool,
    // * 5. 清理历史残留目录 (~/Documents 垃圾目录)
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

// * 流水线执行汇总统计
#[derive(Debug, Default)]
pub struct PatchReport {
    pub app_name: String,
    pub initial_size: u64,
    pub final_size: u64,
    pub inject_status: Option<InjectResult>,
    pub removed_components: Vec<String>,
    pub pruned_locales: Vec<String>,
    pub thinned_count: usize,
    pub restored_real: bool,
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

    debug!("🔍 正在调用 clang 编译动态库: {}", output_dylib.display());

    // * 使用 /usr/bin/clang 进行最高优化级别编译
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
                debug!("🚀 补丁动态库编译成功");
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                error!("❌ clang 编译报错:\n{stderr}");
                Err(format!("clang 编译失败: {stderr}"))
            }
        }
        Err(e) => {
            error!("❌ 无法启动 /usr/bin/clang: {e}");
            Err(format!(
                "无法执行 /usr/bin/clang，请先安装 Xcode Command Line Tools: {e}"
            ))
        }
    }
}

// * 针对指定富途客户端执行全套净化与瘦身流水线
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
        "🚀 开始处理目标客户端: 「{}」 (当前体积: {})",
        app.display_name(),
        format_size(report.initial_size)
    );

    // ! 步骤 0：打补丁前自动终止运行中的客户端进程，杜绝 macOS 内核 vnode 签名缓存冲突
    let proc_name = app.process_name();
    debug!("🔍 检查并终止运行中的客户端进程: {proc_name}");
    let kill_res = Command::new("/usr/bin/killall")
        .arg("-9")
        .arg(proc_name)
        .output();
    if let Ok(out) = kill_res
        && out.status.success()
    {
        info!("🛑 已成功终止正在运行的「{proc_name}」进程，避免内核签名缓存冲突");
        // * 等待系统彻底释放文件句柄
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    // ! 步骤 1：自动恢复历史残留的 .real 文件（彻底终结旧版 bash 包装脚本）
    let real_bin = app.real_binary_path();
    let main_bin = app.binary_path();
    if real_bin.exists() {
        info!(
            "🛠️ 检测到历史残留的备份文件「{}」，正在自动还原为真实主二进制...",
            real_bin.file_name().unwrap_or_default().to_string_lossy()
        );
        if let Err(e) = fs::rename(&real_bin, &main_bin) {
            warn!("⚠️ 直接重命名失败，尝试强制覆写还原: {e}");
            fs::copy(&real_bin, &main_bin).map_err(|e| format!("还原二进制失败: {e}"))?;
            let _ = fs::remove_file(&real_bin);
        }
        report.restored_real = true;
        info!("🚀 历史残留 .real 二进制已成功恢复原位！");
    }

    // * 步骤 2：核心补丁编译与 Mach-O 尾部注入
    if options.core_patch {
        info!("📦 正在编译高性能纯净动态库 (libfutu_patch.dylib)...");
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
        info!("📂 补丁动态库已部署至: 「{}」", target_dylib.display());

        // * 执行纯 Rust Mach-O 尾部加载表注入
        info!("💉 正在对主二进制加载表追加写入 LC_LOAD_DYLIB...");
        match inject_file(&main_bin, "@rpath/libfutu_patch.dylib") {
            Ok(result) => {
                report.inject_status = Some(result);
                match result {
                    InjectResult::Injected => {
                        info!("🚀 已成功追加 @rpath/libfutu_patch.dylib 至二进制加载表最末尾！")
                    }
                    InjectResult::AlreadyPresent => {
                        info!("💡 动态库已位于加载表末尾，无需重复写入")
                    }
                    InjectResult::FixedAndInjected => {
                        info!("🛠️ 成功移除历史错位项并重新写入末尾！")
                    }
                }
            }
            Err(e) => {
                return Err(format!("Mach-O 注入失败: {e}"));
            }
        }
    }

    // * 步骤 3：移除冗余无用组件
    if options.remove_bloatware {
        info!("✂️ 正在移除臃肿冗余组件...");
        match remove_bloatware(&app_path) {
            Ok(removed) => report.removed_components = removed,
            Err(e) => warn!("⚠️ 移除部分冗余组件时遇到错误: {e}"),
        }
    }

    // * 步骤 4：精简 CEF 语言包与快照
    if options.prune_locales {
        info!("🌐 正在精简非必要语言包与多余快照...");
        match prune_locales(&app_path) {
            Ok(pruned) => report.pruned_locales = pruned,
            Err(e) => warn!("⚠️ 精简语言包时遇到错误: {e}"),
        }
    }

    // * 步骤 5：剥离通用架构，提取纯 arm64 二进制
    if options.thin_arm64 {
        info!("🪶 正在检测并使用 lipo 提取纯 arm64 二进制...");
        match thin_arm64_binaries(&app_path) {
            Ok(count) => report.thinned_count = count,
            Err(e) => warn!("⚠️ 提取 arm64 架构时遇到错误: {e}"),
        }
    }

    // * 步骤 6：本地 Ad-hoc 重签名与清理扩展属性
    info!("🔏 正在进行本地 Ad-hoc 重签名...");
    if let Err(e) = resign_app(&app_path) {
        warn!("⚠️ 重签名过程中发生警告: {e}");
    }

    report.final_size = calculate_dir_size(&app_path);
    info!(
        "🎉 「{}」流水线执行完毕！瘦身后体积: {}",
        app.display_name(),
        format_size(report.final_size)
    );

    Ok(report)
}

// * 执行历史残留文档清理
pub fn run_documents_clean() {
    info!("🧹 正在扫描并清理 ~/Documents 历史残留缓存...");
    match clean_user_documents() {
        Ok(paths) => {
            if paths.is_empty() {
                debug!("🔍 未发现 ~/Documents 残留缓存");
            } else {
                info!("🚀 已成功清理 {} 个历史残留目录", paths.len());
            }
        }
        Err(e) => warn!("⚠️ 清理 ~/Documents 时发生错误: {e}"),
    }
}
