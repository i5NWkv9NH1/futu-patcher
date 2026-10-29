// * futu-patcher 编译期构建脚本
// * 在 cargo build 阶段预编译 patches/futu_hook.rs 为 libfutu_patch.dylib 并输出到 OUT_DIR

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=patches/futu_hook.rs");
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = env::var("OUT_DIR").expect("未获取到 OUT_DIR 环境变量");
    let out_dylib = PathBuf::from(&out_dir).join("libfutu_patch.dylib");
    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());

    // * 编译纯 Rust 补丁动态库为 cdylib 并剥离调试符号
    let status = Command::new(&rustc)
        .arg("--edition=2024")
        .arg("--crate-type=cdylib")
        .arg("-O")
        .arg("-C")
        .arg("panic=abort")
        .arg("-C")
        .arg("strip=symbols")
        .arg("-C")
        .arg("link-arg=-Wl,-install_name,@rpath/libfutu_patch.dylib")
        .arg("-l")
        .arg("objc")
        .arg("-l")
        .arg("framework=Foundation")
        .arg("-l")
        .arg("framework=AppKit")
        .arg("patches/futu_hook.rs")
        .arg("-o")
        .arg(&out_dylib)
        .status()
        .expect("无法调用 rustc 预编译动态库");

    if !status.success() {
        panic!("构建期预编译 patches/futu_hook.rs 失败");
    }
}
