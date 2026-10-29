# futu-patcher: macOS Futu Client Purifier and Debloater

[English](./README.md) | [简体中文](./README_CN.md)

> [!NOTE]
> This project is designed for personal use. Core implementation and project architecture were assisted by **Gemini 3.8 Flash** and **Gemini 4 Argon**.

`futu-patcher` is an interactive CLI utility written in **Rust 2024 Edition** designed to debloat, sandbox, and optimize macOS Futu trading clients. It resolves issues regarding background telemetry, Tencent IMSDK dumping data in `~/Documents`, excessive universal binary sizes, and `Cmd+Q` termination lockups.

---

### 1. Key Highlights & Problems Solved

1. **Native Mach-O Injection (Farewell to `.real` Wrapper Scripts)**:
   Completely removes the legacy hack of renaming binaries to `FutuNiuniu.real` and executing a wrapper script. Both `Futubull` and `FutuNiuniu` now use safe, pure-Rust **Mach-O `LC_LOAD_DYLIB` tail injection**. The embedded dylib bootstraps itself seamlessly via `execv`, ensuring `DYLD_INTERPOSE` hooks work properly while preserving the native process identity.
2. **Process Pre-Termination Protection**:
   Automatically terminates running `FTNN` or `FutuNiuniu` instances before applying patches, eliminating macOS kernel vnode code-signature caching conflicts.
3. **Mach-O Header Padding Zero-Check**:
   Strictly validates that destination bytes in the Mach-O header are all zero before appending `LC_LOAD_DYLIB`, preventing any accidental overwrite of subsequent code segments.
4. **Dual Sandboxing for IMSDK and LocalizedString**:
   Intercepts and redirects both `com_tencent_imsdk_data` and `LocalizedString` directories away from `~/Documents` into `~/Library/Application Support/.../SandboxedDocuments`.
5. **Accurate User Home Resolution Under `sudo`**:
   Inspects `SUDO_USER` to resolve the caller's actual `/Users/<username>` home path rather than defaulting to `/var/root`.
6. **Dual-Layer Network Gateway (DNS & IP Interception)**:
   Intercepts `getaddrinfo` and `connect` calls at the POSIX level to drop telemetry hostnames (`collect.*`, `apm.futu`, `crashlytics`, `firebase`, etc.) and block hardcoded direct reporting IPs (`42.193.128.62` and `129.226.79.246`).
7. **Runtime Memory Patching**:
   Automatically patches entry points of reporting engines (`F3CReport`, `libFTApmKit`, `Firebase`, `Crashlytics`) in memory with `RET` instructions to silence analytics.
8. **Cmd+Q 400ms Fast Exit Watchdog**:
   Eliminates Dock freezing upon quit by giving the main runloop 400ms to persist preferences before exiting through the kernel layer.
9. **Comprehensive Debloating & Universal arm64 Thinning**:
   Strips extra CEF language bundles, removes internal Python runtimes and test helpers, and executes `lipo -thin arm64` across all Universal binaries to reclaim hundreds of megabytes.

---

### 2. Client Comparison Matrix

| Dimension | Futubull (Modern Long-Term Version) | FutuNiuniu (Legacy Cocoa Version) |
| :--- | :--- | :--- |
| **Bundle Path** | `/Applications/Futubull.app` | `/Applications/FutuNiuniu.app` |
| **Main Binary** | `FTNN` | `FutuNiuniu` |
| **Tech Stack** | Qt 5/6 + Chromium Embedded Framework (CEF) | Native macOS Cocoa (AppKit / Objective-C) |
| **Official Maintenance** | Actively maintained by Futu | Discontinued (`brew install --cask futubull@legacy`) |
| **Injection Method** | Pure Rust Mach-O `LC_LOAD_DYLIB` appended to load commands | Pure Rust Mach-O `LC_LOAD_DYLIB` appended to load commands |
| **Network Gateway** | DNS blocking + Direct IP blocking | DNS blocking + Direct IP blocking |
| **Memory Hooks** | Patches `F3CReport` & `libFTApmKit` | Patches `F3CReport` & `Firebase`/`GoogleDataTransport` |
| **Disk Savings** | **200 MB ~ 400+ MB** (CEF locales, snapshots, Python, audio drivers, arm64 thinning) | **50 MB ~ 100+ MB** (arm64 thinning, cache cleanup) |
| **Proxy Routing** | `PROCESS-NAME,FTNN,DIRECT` | `PROCESS-NAME,FutuNiuniu,DIRECT` |

---

### 3. Interactive Modules

Running `sudo futu-patcher` without arguments automatically launches an interactive multi-select menu (all enabled by default):

```text
[x] 1. Core Patch
    - Sandbox redirection: Redirect com_tencent_imsdk_data & LocalizedString away from ~/Documents
    - Network gateway: DNS & direct IP blocking for tracking and crash reports
    - Memory hooks: Disable F3CReport, libFTApmKit, Firebase
    - Exit watchdog: 400ms clean quit on Cmd+Q
[x] 2. Remove Bloatware
    - Remove CrashReporter.app, NNPython.app, libFTAutoTest.dylib, and audio drivers
[x] 3. Prune Locales & Snapshots
    - Remove unnecessary CEF locales (af, de, en_GB, fr, ja, ko, ms.lproj) and x86_64 snapshots
[x] 4. Thin arm64 (via lipo)
    - Strip x86_64 architecture from Universal Mach-O binaries on Apple Silicon
[x] 5. Clean Documents
    - Remove residual ~/Documents/com_tencent_imsdk_data and LocalizedString directories
```

After applying modifications, the tool clears quarantine flags (`xattr -cr`), re-signs the application with ad-hoc signatures (`codesign --force --deep -s -`), and prints disk space statistics.

---

### 4. Getting Started

#### 1. Build and Run

Run via Cargo or with the included Fish 4.9 helper script:

```fish
# One-command build and elevated launch with Fish
./run.fish
```

Or using Cargo:

```bash
cargo build --release
sudo ./target/release/futu-patcher
```

#### 2. Non-Interactive Silent Mode

Run across all installed clients with all default optimizations:

```bash
sudo ./target/release/futu-patcher --yes
```

#### 3. CLI Flags

```bash
# Target only Futubull with all defaults
sudo ./target/release/futu-patcher -t futubull -y

# Target only FutuNiuniu with all defaults
sudo ./target/release/futu-patcher -t futuniuniu -y

# Apply core injection patch only, skipping file removal
sudo ./target/release/futu-patcher -y --no-bloat --no-locales --no-thin --no-docs

# Enable verbose logging
sudo ./target/release/futu-patcher -v
```

---

### 5. Repository Structure

```text
futu-patcher/
├── Cargo.toml                  # Rust 2024 CLI project manifest (clap, dialoguer, console)
├── README.md                   # English documentation (GitHub default)
├── README_CN.md                # Simplified Chinese documentation
├── run.fish                    # Fish 4.9 build & run helper
├── patches/                    # Embedded Objective-C injection payloads
│   ├── futubull_patch.m        # Patch source for Futubull (FTNN)
│   └── futuniuniu_patch.m      # Patch source for legacy FutuNiuniu
└── src/
    ├── main.rs                 # CLI entry point: Interactive menu & argument parsing
    ├── macho.rs                # Pure Rust Mach-O LC_LOAD_DYLIB lossless injector
    ├── patcher.rs              # Execution pipeline: kill proc -> restore .real -> compile -> inject -> prune -> lipo -> sign
    └── cleaner.rs              # Component remover, CEF locale pruner, SUDO_USER ~/Documents cleaner
```

---

### 6. Disclaimer

This utility is developed for personal educational, system-cleaning, and software maintenance purposes on local systems only. No warranty is provided.
