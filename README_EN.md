# futu-patcher: macOS Futu Client Patch & Debloat Tool

[简体中文](./README.md) | [English](./README_EN.md)

> [!NOTE]
> This project is designed for personal use. Generated with assistance from **Gemini 3.8 Flash** and **Gemini 4 Argon**.

A clean, interactive CLI utility built with **Rust 2024 Edition** to patch and debloat macOS Futu clients. It addresses telemetry reporting, IMSDK data clutter in `~/Documents`, excessive bundle sizes, and `Cmd+Q` termination freezes.

---

### 1. Client Comparison

| Dimension | Futubull (Modern Version) | FutuNiuniu (Classic Version) |
| :--- | :--- | :--- |
| **Bundle Path** | `/Applications/Futubull.app` | `/Applications/FutuNiuniu.app` |
| **Main Binary** | `FTNN` | `FutuNiuniu` |
| **Tech Stack** | Qt 5/6 + CEF (Chromium) | Native macOS Cocoa (AppKit) |
| **Maintenance** | Actively maintained by Futu | Discontinued (`brew install --cask futubull@legacy`) |
| **Injection** | Pure Rust Mach-O tail `LC_LOAD_DYLIB` | Pure Rust Mach-O tail `LC_LOAD_DYLIB` |
| **Telemetry Hooks** | Patches `F3CReport` & `libFTApmKit` | Patches `F3CReport` & `Firebase` |
| **Space Saved** | ~**918 MB** (`1.84 GB -> 966 MB`, reduced by ~`48.7%`) | ~**178 MB** (`457 MB -> 279 MB`, reduced by ~`39.0%`) |

---

### 2. Core Features

1. **Patch Injection**: Sandboxes `com_tencent_imsdk_data` & `LocalizedString` away from `~/Documents`, drops telemetry traffic, silences reporting in memory, and adds a 400ms delayed exit mechanism for smooth `Cmd+Q` exit.
2. **Remove Bloatware**: Removes `CrashReporter.app`, `NNPython.app`, `libFTAutoTest.dylib`, and virtual audio plugins.
3. **Prune Locales & Snapshots**: Removes redundant CEF language bundles and unused x86_64 V8 context snapshots.
4. **Thin arm64 Architecture**: Invokes `lipo -thin arm64` across all Universal binaries to strip unused `x86_64` code.
5. **Clean Documents**: Resolves real user home path via `SUDO_USER` and cleans legacy folders in `~/Documents`.

---

### 3. Build & Run

```bash
# Build release binary
cargo build --release

# Interactive run (all selected by default)
sudo ./target/release/futu-patcher

# Non-interactive silent mode for all installed clients
sudo ./target/release/futu-patcher -y

# Diagnostics mode: inspect Futubull bundle layout and symbols (no root required)
./target/release/futu-patcher -i
```

---

### 4. Proxy Routing Rules

The native process names remain unchanged after patching. Configure Surge / Clash rules as follows:

```text
PROCESS-NAME,FTNN,DIRECT
PROCESS-NAME,FutuNiuniu,DIRECT
```

---

### 5. Future Updates & Maintenance

When Futubull updates, use diagnostics mode to inspect bundle changes:

1. **Inspect Bundle**: Run `./target/release/futu-patcher -i`. The tool displays bundle version, sizes of internal components, `FTNN` dependencies, and telemetry symbols in `F3CReport` and `libFTApmKit`.
2. **Review Symbols**: If Futu changed telemetry function names, update the matched symbol names in `patches/futubull_patch.m`.
3. **Re-apply Patch**: Run `sudo futu-patcher` to compile and apply patches.
