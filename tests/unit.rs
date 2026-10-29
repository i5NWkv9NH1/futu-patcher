// * futu-patcher 独立单元测试集合

use clap::Parser;
use futu_patcher::cleaner::{format_size, get_user_home_dir};
use futu_patcher::macho::{
    CPU_TYPE_ARM64, FAT_MAGIC, InjectResult, MH_MAGIC_64, MachoError, find_arm64_slice_offset,
    inject_slice,
};
use futu_patcher::{CliArgs, TargetArg};

// * 构造基础 64 位 Mach-O 内存切片用于测试
fn create_mock_macho_64() -> Vec<u8> {
    let mut buf = vec![0u8; 4096];
    buf[0..4].copy_from_slice(&MH_MAGIC_64.to_le_bytes()); // * magic
    buf[4..8].copy_from_slice(&CPU_TYPE_ARM64.to_le_bytes()); // * cputype
    buf[8..12].copy_from_slice(&0x02u32.to_le_bytes()); // * cpusubtype
    buf[12..16].copy_from_slice(&0x02u32.to_le_bytes()); // * filetype (MH_EXECUTE)
    buf[16..20].copy_from_slice(&0u32.to_le_bytes()); // * ncmds = 0
    buf[20..24].copy_from_slice(&0u32.to_le_bytes()); // * sizeofcmds = 0
    buf[24..28].copy_from_slice(&0u32.to_le_bytes()); // * flags
    buf[28..32].copy_from_slice(&0u32.to_le_bytes()); // * reserved
    buf
}

#[test]
fn test_inject_clean_macho() {
    let mut macho = create_mock_macho_64();
    let target = "@rpath/libfutu_patch.dylib";

    // * 首次注入测试
    let res = inject_slice(&mut macho, 0, target).expect("首次注入应成功");
    assert_eq!(res, InjectResult::Injected);

    // * 校验头部字段更新
    let ncmds = u32::from_le_bytes(macho[16..20].try_into().unwrap());
    let sizeofcmds = u32::from_le_bytes(macho[20..24].try_into().unwrap());
    assert_eq!(ncmds, 1);
    let expected_size = ((24 + target.len() + 1 + 7) & !7) as u32;
    assert_eq!(sizeofcmds, expected_size);

    // * 重复注入测试（应幂等跳过）
    let res2 = inject_slice(&mut macho, 0, target).expect("重复注入应无报错");
    assert_eq!(res2, InjectResult::AlreadyPresent);
}

#[test]
fn test_inject_misplaced_repair() {
    let mut macho = create_mock_macho_64();
    let target = "@rpath/libfutu_patch.dylib";

    // * 注入目标 dylib
    inject_slice(&mut macho, 0, target).unwrap();

    // * 在其后再伪造追加一个其他加载命令，模拟 target 不在末尾的场景
    let dummy_size = 24u32;
    let offset = 32 + u32::from_le_bytes(macho[20..24].try_into().unwrap()) as usize;
    macho[offset..offset + 4].copy_from_slice(&0x1Bu32.to_le_bytes()); // * LC_LOAD_WEAK_DYLIB
    macho[offset + 4..offset + 8].copy_from_slice(&dummy_size.to_le_bytes());
    macho[offset + 8..offset + 12].copy_from_slice(&24u32.to_le_bytes());

    // * 更新头部
    let mut ncmds = u32::from_le_bytes(macho[16..20].try_into().unwrap()) + 1;
    let mut sizeofcmds = u32::from_le_bytes(macho[20..24].try_into().unwrap()) + dummy_size;
    macho[16..20].copy_from_slice(&ncmds.to_le_bytes());
    macho[20..24].copy_from_slice(&sizeofcmds.to_le_bytes());

    // ! 此时 target 位于 index 0，而非末尾 index 1；再次运行注入，应触发错位自动修复
    let res = inject_slice(&mut macho, 0, target).expect("修复错位项应成功");
    assert_eq!(res, InjectResult::FixedAndInjected);

    ncmds = u32::from_le_bytes(macho[16..20].try_into().unwrap());
    sizeofcmds = u32::from_le_bytes(macho[20..24].try_into().unwrap());
    assert_eq!(ncmds, 2);
    let expected_target_size = ((24 + target.len() + 1 + 7) & !7) as u32;
    assert_eq!(sizeofcmds, dummy_size + expected_target_size);
}

#[test]
fn test_inject_padding_not_zero_security_check() {
    let mut macho = create_mock_macho_64();
    let target = "@rpath/libfutu_patch.dylib";

    // ! 人为将加载命令结束后的填充区域填入非零数据，模拟紧邻代码段或损坏的 Padding 区域
    let cmds_end = 32; // * 因为目前 sizeofcmds 为 0
    macho[cmds_end + 10] = 0xAA;

    // ! 触发安全校验拒绝注入
    let res = inject_slice(&mut macho, 0, target);
    assert!(matches!(res, Err(MachoError::HeaderPaddingNotZero)));
}

#[test]
fn test_find_arm64_slice_in_fat() {
    let mut fat = vec![0u8; 8192];
    fat[0..4].copy_from_slice(&FAT_MAGIC.to_be_bytes());
    fat[4..8].copy_from_slice(&2u32.to_be_bytes()); // * 2 个架构

    // * 第 1 个架构：x86_64 (cputype = 0x01000007, offset = 0x1000)
    fat[8..12].copy_from_slice(&0x01000007u32.to_be_bytes());
    fat[16..20].copy_from_slice(&0x1000u32.to_be_bytes());

    // * 第 2 个架构：arm64 (cputype = 0x0100000c, offset = 0x2000)
    fat[28..32].copy_from_slice(&CPU_TYPE_ARM64.to_be_bytes());
    fat[36..40].copy_from_slice(&0x2000u32.to_be_bytes());

    let offset = find_arm64_slice_offset(&fat).expect("应正确解析 FAT 中的 arm64 切片");
    assert_eq!(offset, 0x2000);
}

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

#[test]
fn test_cli_default_options() {
    let args = CliArgs::parse_from(["futu-patcher"]);
    assert!(!args.yes);
    assert!(args.target.is_none());
    let opts = args.to_patch_options();
    assert!(opts.core_patch);
    assert!(opts.remove_bloatware);
    assert!(opts.prune_locales);
    assert!(opts.thin_arm64);
    assert!(opts.clean_documents);
}

#[test]
fn test_cli_flags_toggle() {
    let args = CliArgs::parse_from([
        "futu-patcher",
        "--yes",
        "--target",
        "futubull",
        "--no-bloat",
        "--no-docs",
    ]);
    assert!(args.yes);
    assert_eq!(args.target, Some(TargetArg::Futubull));
    let opts = args.to_patch_options();
    assert!(opts.core_patch);
    assert!(!opts.remove_bloatware);
    assert!(opts.prune_locales);
    assert!(opts.thin_arm64);
    assert!(!opts.clean_documents);
}
