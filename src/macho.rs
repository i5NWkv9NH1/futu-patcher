// * 纯 Rust Mach-O LC_LOAD_DYLIB 注入器
// * 替代原 futu_inject.c，提供内存安全、头部溢出防御与尾部追加功能

use std::fs;
use std::path::Path;
use tracing::{debug, error, warn};

// * Mach-O 与 FAT 核心魔数与常量定义
pub const MH_MAGIC_64: u32 = 0xfeedfacf;
pub const MH_CIGAM_64: u32 = 0xcffaedfe;
pub const FAT_MAGIC: u32 = 0xcafebabe;
pub const FAT_CIGAM: u32 = 0xbebafeca;
pub const CPU_TYPE_ARM64: u32 = 0x0100000c;
pub const LC_LOAD_DYLIB: u32 = 0x0000000c;

// * 注入执行结果枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectResult {
    // * 动态库已成功追加写入加载表末尾
    Injected,
    // * 动态库已在加载表末尾，无需重复操作
    AlreadyPresent,
    // * 修复了早期版本插在顶部的错位项，并重新追加至末尾
    FixedAndInjected,
}

// * Mach-O 操作异常枚举
#[derive(Debug, thiserror::Error)]
pub enum MachoError {
    #[error("文件 I/O 异常: {0}")]
    Io(#[from] std::io::Error),
    #[error("不支持的二进制格式 (魔数: 0x{0:x})")]
    UnsupportedMagic(u32),
    #[error("未在通用二进制中找到 arm64 架构切片")]
    Arm64SliceNotFound,
    #[error("Mach-O 头部或加载命令损坏: {0}")]
    Corrupted(String),
    #[error("二进制头部空闲填充空间不足，无法追加加载命令")]
    NoSpaceAvailable,
    #[error("Mach-O 头部预留填充区包含非零数据，为保护后续代码段已终止写入")]
    HeaderPaddingNotZero,
}

// * 解析 Mach-O 头部以获取 arm64 切片的文件偏移量
pub fn find_arm64_slice_offset(data: &[u8]) -> Result<usize, MachoError> {
    // ! 检查文件长度是否至少包含 4 字节魔数
    if data.len() < 4 {
        return Err(MachoError::Corrupted("文件长度小于 4 字节".to_string()));
    }

    let magic = u32::from_ne_bytes([data[0], data[1], data[2], data[3]]);

    if magic == FAT_MAGIC || magic == FAT_CIGAM {
        debug!("🔍 检测到 Universal 通用二进制 (FAT 容器)");
        // ! FAT 头部中所有整数字段一律按大端字节序 (Big-Endian) 存储
        if data.len() < 8 {
            return Err(MachoError::Corrupted("FAT 头部长度不足".to_string()));
        }
        let nfat_arch = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
        let mut offset = 8;

        for _ in 0..nfat_arch {
            if offset + 20 > data.len() {
                return Err(MachoError::Corrupted("FAT 架构描述表截断".to_string()));
            }
            let cputype = u32::from_be_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            let slice_offset = u32::from_be_bytes([
                data[offset + 8],
                data[offset + 9],
                data[offset + 10],
                data[offset + 11],
            ]) as usize;

            if cputype == CPU_TYPE_ARM64 {
                debug!("🔍 成功定位 arm64 切片，偏移地址: 0x{slice_offset:x}");
                return Ok(slice_offset);
            }
            offset += 20;
        }
        Err(MachoError::Arm64SliceNotFound)
    } else if magic == MH_MAGIC_64 || magic == MH_CIGAM_64 {
        debug!("🔍 检测到单架构 Thin 64位 Mach-O 二进制");
        Ok(0)
    } else {
        error!("❌ 未知或不支持的二进制魔数: 0x{magic:x}");
        Err(MachoError::UnsupportedMagic(magic))
    }
}

// * 在指定的 Mach-O 字节流中对 arm64 切片注入 LC_LOAD_DYLIB
pub fn inject_slice(
    data: &mut [u8],
    slice_offset: usize,
    dylib_path: &str,
) -> Result<InjectResult, MachoError> {
    // ! 校验切片头部尺寸（mach_header_64 固定占用 32 字节）
    if data.len() < slice_offset + 32 {
        return Err(MachoError::Corrupted(
            "切片数据小于 mach_header_64 尺寸".to_string(),
        ));
    }

    let magic = u32::from_le_bytes([
        data[slice_offset],
        data[slice_offset + 1],
        data[slice_offset + 2],
        data[slice_offset + 3],
    ]);

    if magic != MH_MAGIC_64 {
        return Err(MachoError::Corrupted(format!(
            "切片魔数非 MH_MAGIC_64: 0x{magic:x}"
        )));
    }

    // * 解析 mach_header_64 的 ncmds 与 sizeofcmds 字段
    let mut ncmds = u32::from_le_bytes([
        data[slice_offset + 16],
        data[slice_offset + 17],
        data[slice_offset + 18],
        data[slice_offset + 19],
    ]);
    let mut sizeofcmds = u32::from_le_bytes([
        data[slice_offset + 20],
        data[slice_offset + 21],
        data[slice_offset + 22],
        data[slice_offset + 23],
    ]);

    let cmds_start = slice_offset + 32;
    let mut cmds_end = cmds_start + sizeofcmds as usize;

    if data.len() < cmds_end {
        return Err(MachoError::Corrupted(
            "加载命令总长度超出切片边界".to_string(),
        ));
    }

    // * 计算新注入命令的对齐长度（struct dylib_command 为 24 字节，字符串需含末尾 \0 并按 8 字节对齐）
    let path_bytes = dylib_path.as_bytes();
    let path_len_with_null = path_bytes.len() + 1;
    let add_cmdsize = ((24 + path_len_with_null + 7) & !7) as u32;

    let mut cursor = cmds_start;
    let mut fixed_misplaced = false;

    // * 遍历现有 Load Commands，检测是否已存在目标 dylib 或存在早期错位项
    let mut i = 0;
    while i < ncmds {
        if cursor + 8 > cmds_end {
            return Err(MachoError::Corrupted("遍历加载命令时发生越界".to_string()));
        }

        let cmd = u32::from_le_bytes([
            data[cursor],
            data[cursor + 1],
            data[cursor + 2],
            data[cursor + 3],
        ]);
        let cmdsize = u32::from_le_bytes([
            data[cursor + 4],
            data[cursor + 5],
            data[cursor + 6],
            data[cursor + 7],
        ]) as usize;

        if cmdsize < 8 || cursor + cmdsize > cmds_end {
            return Err(MachoError::Corrupted(format!(
                "加载命令尺寸异常: cmd=0x{cmd:x}, size={cmdsize}"
            )));
        }

        if cmd == LC_LOAD_DYLIB {
            // ! 解析 dylib 路径偏移量 (在 dylib_command 偏移 8 字节处)
            if cmdsize >= 24 {
                let name_offset = u32::from_le_bytes([
                    data[cursor + 8],
                    data[cursor + 9],
                    data[cursor + 10],
                    data[cursor + 11],
                ]) as usize;

                if name_offset < cmdsize && cursor + name_offset < cmds_end {
                    let str_bytes = &data[cursor + name_offset..cursor + cmdsize];
                    let parsed_name = std::str::from_utf8(
                        str_bytes.split(|&b| b == 0).next().unwrap_or_default(),
                    )
                    .unwrap_or_default();

                    if parsed_name == dylib_path {
                        // * 如果位于最末尾一条，说明已经打好补丁
                        if i == ncmds - 1 {
                            debug!("🔍 补丁已存在于加载表最末尾，无需重复写入");
                            return Ok(InjectResult::AlreadyPresent);
                        }

                        // ! 如果不在末尾，将其从原位置抹去并前移后续命令
                        warn!("⚠️ 检测到旧版错位项，正在剔除并重排加载命令表...");
                        let copy_src = cursor + cmdsize;
                        let copy_len = cmds_end - copy_src;
                        data.copy_within(copy_src..copy_src + copy_len, cursor);

                        ncmds -= 1;
                        sizeofcmds -= cmdsize as u32;
                        cmds_end -= cmdsize;

                        // * 抹平尾部腾出的多余空间
                        data[cmds_end..cmds_end + cmdsize].fill(0);
                        fixed_misplaced = true;
                        // ! 重新在当前光标处检测，不递增 i
                        continue;
                    }
                }
            }
        }

        cursor += cmdsize;
        i += 1;
    }

    // ! 检查头部填充空间：确保 cmds_end + add_cmdsize 不会越界
    if data.len() < cmds_end + add_cmdsize as usize {
        return Err(MachoError::NoSpaceAvailable);
    }

    // ! 关键安全审计门禁：校验即将写入的区域是否全为 0（确保处于合法的 Header Padding 空白区，绝不覆盖后续代码段）
    let target_range = cmds_end..cmds_end + add_cmdsize as usize;
    if data[target_range.clone()].iter().any(|&b| b != 0) {
        error!("❌ Mach-O 头部预留填充区包含非零数据，为保护后续代码段已终止写入！");
        return Err(MachoError::HeaderPaddingNotZero);
    }

    // * 构造 struct dylib_command 字节流 (24 字节)
    let mut new_cmd = vec![0u8; add_cmdsize as usize];
    new_cmd[0..4].copy_from_slice(&LC_LOAD_DYLIB.to_le_bytes());
    new_cmd[4..8].copy_from_slice(&add_cmdsize.to_le_bytes());
    new_cmd[8..12].copy_from_slice(&24u32.to_le_bytes()); // * name.offset = 24
    new_cmd[12..16].copy_from_slice(&2u32.to_le_bytes()); // * timestamp = 2
    new_cmd[16..20].copy_from_slice(&0x00010000u32.to_le_bytes()); // * current_version = 1.0.0
    new_cmd[20..24].copy_from_slice(&0x00010000u32.to_le_bytes()); // * compatibility_version = 1.0.0

    // * 拷贝动态库路径字符串及结尾 null 字节
    new_cmd[24..24 + path_bytes.len()].copy_from_slice(path_bytes);
    new_cmd[24 + path_bytes.len()] = 0;

    // * 写入新命令至加载表尾部
    data[target_range].copy_from_slice(&new_cmd);

    // * 更新 mach_header_64 的 ncmds 与 sizeofcmds
    ncmds += 1;
    sizeofcmds += add_cmdsize;

    data[slice_offset + 16..slice_offset + 20].copy_from_slice(&ncmds.to_le_bytes());
    data[slice_offset + 20..slice_offset + 24].copy_from_slice(&sizeofcmds.to_le_bytes());

    if fixed_misplaced {
        debug!("🔍 修复错位项并追加「{dylib_path}」至加载表末尾");
        Ok(InjectResult::FixedAndInjected)
    } else {
        debug!("🔍 成功追加「{dylib_path}」至加载表末尾");
        Ok(InjectResult::Injected)
    }
}

// * 对指定二进制文件执行加载表注入
pub fn inject_file<P: AsRef<Path>>(path: P, dylib_path: &str) -> Result<InjectResult, MachoError> {
    let path_ref = path.as_ref();
    debug!("🔍 读取并解析二进制文件: {}", path_ref.display());

    let mut data = fs::read(path_ref)?;
    let slice_offset = find_arm64_slice_offset(&data)?;
    let result = inject_slice(&mut data, slice_offset, dylib_path)?;

    if result != InjectResult::AlreadyPresent {
        fs::write(path_ref, &data)?;
        debug!("🔍 写入更新后的二进制数据完成: {}", path_ref.display());
    }

    Ok(result)
}
