// * 冗余组件清理、CEF 语言包精简与 ~/Documents 历史缓存清理模块

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info, warn};
use walkdir::WalkDir;

use crate::macho::{CPU_TYPE_ARM64, FAT_CIGAM, FAT_MAGIC};

// * 获取真实用户的主目录（防范 sudo 提权运行导致的 $HOME 指向 /var/root）
pub fn get_user_home_dir() -> PathBuf {
    // ! 优先检查 sudo 环境变量 SUDO_USER，获取真正发起命令的普通用户主目录
    if let Ok(sudo_user) = std::env::var("SUDO_USER") {
        let trimmed = sudo_user.trim();
        if !trimmed.is_empty() {
            let candidate = PathBuf::from(format!("/Users/{trimmed}"));
            if candidate.exists() {
                debug!(
                    "🔍 检测到 sudo 提权环境，真实用户目录: {}",
                    candidate.display()
                );
                return candidate;
            }
        }
    }

    // ? 回退使用当前环境的 HOME 环境变量
    if let Ok(home) = std::env::var("HOME") {
        let path = PathBuf::from(home);
        if path.exists() {
            return path;
        }
    }

    PathBuf::from("/Users")
}

// * 计算指定目录或文件的总磁盘占用（字节数）
pub fn calculate_dir_size<P: AsRef<Path>>(path: P) -> u64 {
    let mut total_size = 0u64;
    for entry in WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
        if let Ok(meta) = entry.metadata()
            && meta.is_file()
        {
            total_size += meta.len();
        }
    }
    total_size
}

// * 将字节数转换为直观的人类可读格式 (MB/GB)
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

// * 1. 移除冗余组件 (CrashReporter、NNPython、AutoTest、声卡驱动)
pub fn remove_bloatware(app_path: &Path) -> Result<Vec<String>, std::io::Error> {
    let mut removed = Vec::new();
    let fw_dir = app_path.join("Contents/Frameworks");

    let targets = [
        fw_dir.join("CrashReporter.app"),
        fw_dir.join("NNPython.app"),
        fw_dir.join("FTNN.real Helper.app"),
        fw_dir.join("libFTAutoTest.dylib"),
        fw_dir.join(
            "TXLiteAVSDK_TRTC_Mac.framework/Versions/A/Resources/PlugIns/TRTCAudioPlugin.driver",
        ),
    ];

    for target in &targets {
        if target.exists() {
            // ! 区分文件与目录分别移除
            if target.is_dir() {
                fs::remove_dir_all(target)?;
            } else {
                fs::remove_file(target)?;
            }
            let name = target
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            info!("✂️ 已移除冗余组件: 「{name}」");
            removed.push(name);
        }
    }

    Ok(removed)
}

// * 2. 精简 CEF 冗余语言包与多余 x86_64 快照
pub fn prune_locales(app_path: &Path) -> Result<Vec<String>, std::io::Error> {
    let mut pruned = Vec::new();
    let cef_res = app_path
        .join("Contents/Frameworks/Chromium Embedded Framework.framework/Versions/A/Resources");

    if !cef_res.exists() {
        debug!("🔍 未检测到 CEF Resources 目录，跳过语言包精简");
        return Ok(pruned);
    }

    // * 需要移除的非必要语言包列表
    let unused_locales = [
        "af.lproj",
        "de.lproj",
        "en_GB.lproj",
        "fr.lproj",
        "ja.lproj",
        "ko.lproj",
        "ms.lproj",
    ];

    for locale in &unused_locales {
        let locale_path = cef_res.join(locale);
        if locale_path.exists() {
            fs::remove_dir_all(&locale_path)?;
            pruned.push(locale.to_string());
            info!("✂️ 已移除多余语言包: 「{locale}」");
        }
    }

    // * 移除多余的 x86_64 V8 快照
    let x86_snapshot = cef_res.join("v8_context_snapshot.x86_64.bin");
    if x86_snapshot.exists() {
        fs::remove_file(&x86_snapshot)?;
        pruned.push("v8_context_snapshot.x86_64.bin".to_string());
        info!("✂️ 已移除冗余快照: 「v8_context_snapshot.x86_64.bin」");
    }

    Ok(pruned)
}

// * 3. 提取纯 arm64 架构 (通过 lipo -thin arm64 剥离 x86_64)
pub fn thin_arm64_binaries(app_path: &Path) -> Result<usize, std::io::Error> {
    // ! 仅在 Apple Silicon (arm64/aarch64) 架构上执行
    if std::env::consts::ARCH != "aarch64" {
        warn!("⚠️ 当前系统非 arm64 架构，跳过 lipo 瘦身");
        return Ok(0);
    }

    let mut count = 0;
    debug!("🔍 开始扫描 App 包内所有通用二进制文件...");

    for entry in WalkDir::new(app_path).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        // ! 跳过符号链接与非普通文件
        if !path.is_file() || entry.path_is_symlink() {
            continue;
        }

        // * 读取前 8 字节快速检验是否为 FAT 通用二进制
        let Ok(mut file) = fs::File::open(path) else {
            continue;
        };
        use std::io::Read;
        let mut header = [0u8; 8];
        if file.read_exact(&mut header).is_err() {
            continue;
        }
        drop(file);

        let magic = u32::from_ne_bytes([header[0], header[1], header[2], header[3]]);
        if magic != FAT_MAGIC && magic != FAT_CIGAM {
            continue;
        }

        // * 进一步检验是否包含 arm64 架构切片
        let Ok(data) = fs::read(path) else {
            continue;
        };
        if data.len() < 8 {
            continue;
        }
        let nfat_arch = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
        let mut has_arm64 = false;
        let mut offset = 8;
        for _ in 0..nfat_arch {
            if offset + 20 > data.len() {
                break;
            }
            let cputype = u32::from_be_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            if cputype == CPU_TYPE_ARM64 {
                has_arm64 = true;
                break;
            }
            offset += 20;
        }

        if !has_arm64 {
            continue;
        }

        // * 调用系统的 lipo -thin arm64 进行原地瘦身
        let status = Command::new("/usr/bin/lipo")
            .arg("-thin")
            .arg("arm64")
            .arg(path)
            .arg("-output")
            .arg(path)
            .status();

        match status {
            Ok(s) if s.success() => {
                count += 1;
                debug!("🪶 成功转换为纯 arm64: {}", path.display());
            }
            _ => {
                warn!("⚠️ lipo 转换失败: {}", path.display());
            }
        }
    }

    info!("🪶 已成功将 {count} 个通用二进制提取为纯 arm64 格式！");
    Ok(count)
}

// * 4. 清理 ~/Documents 残留垃圾目录
pub fn clean_user_documents() -> Result<Vec<PathBuf>, std::io::Error> {
    let mut cleaned = Vec::new();
    let home = get_user_home_dir();
    let docs = home.join("Documents");

    let targets = [
        docs.join("com_tencent_imsdk_data"),
        docs.join("LocalizedString"),
    ];

    for target in &targets {
        if target.exists() {
            fs::remove_dir_all(target)?;
            cleaned.push(target.clone());
            info!("🧹 已清理用户目录残留: 「{}」", target.display());
        }
    }

    Ok(cleaned)
}

// * 5. 清除属性与本地 Ad-hoc 代码重签名
pub fn resign_app(app_path: &Path) -> Result<(), std::io::Error> {
    info!("🔏 正在清除 App 隔离扩展属性...");
    let _ = Command::new("/usr/bin/xattr")
        .arg("-cr")
        .arg(app_path)
        .status();

    info!("🔏 正在执行本地 Ad-hoc 递归重签名...");
    let status = Command::new("/usr/bin/codesign")
        .arg("--force")
        .arg("--deep")
        .arg("-s")
        .arg("-")
        .arg(app_path)
        .status()?;

    if !status.success() {
        warn!("⚠️ codesign 退出状态非零，请确认是否有足够权限");
    } else {
        info!("🚀 本地重签名顺利完成！");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(500), "500 B");
        assert_eq!(format_size(2048), "2.0 KB");
        assert_eq!(format_size(10 * 1024 * 1024), "10.0 MB");
    }

    #[test]
    fn test_get_user_home_dir_fallback() {
        let home = get_user_home_dir();
        assert!(home.is_absolute());
    }
}
