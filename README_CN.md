# futu-patcher: macOS 富途客户端净化与极致瘦身工具

[English](./README.md) | [简体中文](./README_CN.md)

> [!NOTE]
> 本项目为个人自用工具，核心代码与工程架构由 **Gemini 3.8 Flash** 与 **Gemini 4 Argon** 协同协助生成。

`futu-patcher` 是一套基于 **Rust 2024 Edition** 构建的 macOS 富途客户端交互式净化与瘦身 CLI 工具。彻底解决富途客户端在 macOS 上的后台高频埋点上报、IMSDK 垃圾污染 `~/Documents`、二进制体积臃肿以及 `Cmd+Q` 退出卡死 Dock 栏等体验痛点。

---

### 一、 核心痛点与工程加固

1. **废除历史「.real」改名黑魔法，统一 Mach-O 注入**：
   旧版 `FutuNiuniu` 彻底告别重命名二进制写 bash 启动脚本的脆弱方案，全面统一为使用纯 Rust 实现的 **Mach-O `LC_LOAD_DYLIB` 尾部无损注入**。并在补丁构造函数中实现优雅的 `execv` 自举，确保 `DYLD_INTERPOSE` 符号拦截生效的同时，保持官方原生进程名不变。
2. **打补丁前自动终止运行中的客户端进程**：
   在修改二进制、执行 lipo 瘦身或 Ad-hoc 重签名之前，自动终止正在运行的 `FTNN` 或 `FutuNiuniu` 进程，彻底防范 macOS 内核 vnode 签名缓存冲突。
3. **Mach-O 头部预留填充区零值安全检查**：
   在向 `mach_header_64` 的 Load Commands 尾部追加 `LC_LOAD_DYLIB` 前，严密校验写入区域必须全为 0（确保处于合法的 Header Padding 空白填充区），坚决杜绝因空间不足覆写后续代码段。
4. **腾讯 IMSDK 与本地化垃圾双重沙盒隔离**：
   腾讯 IMSDK 会默认在 `~/Documents/com_tencent_imsdk_data` 疯狂倾倒缓存，且会在 `~/Documents/LocalizedString` 生成冗余残留。本工具通过底层符号拦截将两者全部重定向至 `~/Library/Application Support/.../SandboxedDocuments`。
5. **修复 sudo 环境下的 $HOME 路径漂移**：
   在以管理员权限运行时，智能读取 `SUDO_USER` 映射真实用户主目录（`/Users/<SUDO_USER>`），避免误将缓存清理指向 `/var/root`。
6. **DNS 解析与直连 IP 双重网闸拦截**：
   在底层拦截 `getaddrinfo` 与 `connect`，直接封杀各类埋点域名（`collect.*`、`apm.futu`、`crashlytics`、`firebase` 等）及硬编码直连 IP（`42.193.128.62` 与 `129.226.79.246`）。
7. **C++ 与 Objective-C 内存级拦截**：
   运行时自动将 `F3CReport`、`libFTApmKit`、`Firebase`、`Crashlytics` 的关键调度与监控函数入口覆写为 `RET` 指令，杜绝后台无意义的性能统计与追踪。
8. **Cmd+Q 秒退看门狗**：
   解决退出富途时因静态析构与网络刷新导致的 Dock 栏图标死锁问题，预留 400ms 同步必要偏好设置后直接内核级秒退。
9. **深度精简与 Universal arm64 架构瘦身**：
   一键剔除 CEF 冗余多国语言包、移除内置 Python 和测试组件、调用 `lipo -thin arm64` 剥离 `x86_64` 架构，立竿见影释放数百兆磁盘空间。

---

### 二、 富途新旧客户端对比表

| 特性维度 | Futubull (新版长期维护版) | FutuNiuniu (旧版 legacy 原生版) |
| :--- | :--- | :--- |
| **安装路径** | `/Applications/Futubull.app` | `/Applications/FutuNiuniu.app` |
| **主二进制** | `FTNN` | `FutuNiuniu` |
| **技术栈** | Qt 5/6 + CEF (Chromium Embedded Framework) | 原生 macOS Cocoa (AppKit / Objective-C) |
| **官方维护状态** | 官方主力维护，持续更新迭代 | 官方已停止维护更新 (`brew install --cask futubull@legacy`) |
| **注入实现方式** | 纯 Rust 解析 Mach-O，尾部追加 `LC_LOAD_DYLIB` | 纯 Rust 解析 Mach-O，尾部追加 `LC_LOAD_DYLIB` (彻底抛弃 `.real`) |
| **网络网闸拦截** | DNS 阻断 + 直连 IP 拒连 | DNS 阻断 + 直连 IP 拒连 |
| **内存拦截策略** | 内存屏蔽 `F3CReport` 与 `libFTApmKit` | 内存屏蔽 `F3CReport` 与 `Firebase`/`GoogleDataTransport` |
| **精简瘦身收益** | **200MB ~ 400MB+** (移除 CEF 语言包、快照、Python、音频驱动、剥离 x86_64) | **50MB ~ 100MB+** (剥离 x86_64 切片、清理冗余缓存) |
| **分流进程名** | `PROCESS-NAME,FTNN,DIRECT` | `PROCESS-NAME,FutuNiuniu,DIRECT` |

---

### 三、 交互式菜单与五大模块说明

直接执行 `sudo futu-patcher`，工具将自动扫描本地已安装客户端并进入终端交互选择界面（默认全选）：

```text
[x] 1. 核心注入补丁 (Core Patch)
    - 路径隔离：将 com_tencent_imsdk_data 与 LocalizedString 隔离至 Application Support
    - 拦截网闸：DNS 与直连 IP 双重封杀埋点上报
    - 内存补丁：运行时禁用 F3CReport、libFTApmKit、Firebase
    - 秒退看门狗：Cmd+Q 400ms 内快速退出，杜绝 Dock 栏假死
[x] 2. 移除冗余组件 (Remove Bloatware)
    - 针对 Futubull 剔除 CrashReporter.app、NNPython.app、libFTAutoTest.dylib、声卡驱动
[x] 3. 精简语言包与快照 (Prune Locales)
    - 移除 CEF 冗余语言包 (af, de, en_GB, fr, ja, ko, ms.lproj) 及 x86_64 快照
[x] 4. 提取纯 arm64 架构 (Thin arm64 via lipo)
    - 遍历 App 包内所有 Universal 通用二进制，剥离 x86_64 架构，缩减体积
[x] 5. 清理历史残留目录 (Clean Documents)
    - 智能获取 SUDO_USER 真实主目录，清理 ~/Documents 下遗留的缓存文件夹
```

执行完毕后会自动调用系统的 `xattr -cr` 清除隔离属性，并使用 `codesign --force --deep -s -` 执行本地 Ad-hoc 自签名，最后输出详细的瘦身体积前后对比统计报表。

---

### 四、 快速上手

#### 1. 编译与一键运行

你可以通过 Cargo 构建，也可以使用项目内置的 Fish 4.9 脚本一键提权运行：

```fish
# 使用 run.fish 一键构建并提权运行
./run.fish
```

或使用标准 Cargo 指令：

```bash
cargo build --release
sudo ./target/release/futu-patcher
```

#### 2. 非交互静默模式 (全自动跑完所有已安装客户端)

```bash
sudo ./target/release/futu-patcher --yes
```

#### 3. CLI 参数灵活定制

```bash
# 仅对新版 Futubull 执行默认全套净化
sudo ./target/release/futu-patcher -t futubull -y

# 仅对旧版 FutuNiuniu 执行默认全套净化
sudo ./target/release/futu-patcher -t futuniuniu -y

# 仅注入核心补丁，不移除任何组件或精简语言包
sudo ./target/release/futu-patcher -y --no-bloat --no-locales --no-thin --no-docs

# 开启底层详细调试日志
sudo ./target/release/futu-patcher -v
```

---

### 五、 项目源码组织结构

```text
futu-patcher/
├── Cargo.toml                  # Rust 2024 CLI 项目配置文件 (clap, dialoguer, console)
├── README.md                   # 英文说明文档 (GitHub 默认首页)
├── README_CN.md                # 简体中文说明文档
├── run.fish                    # 兼容 Fish 4.9 的极简一键构建运行脚本
├── patches/                    # 存放 Objective-C 注入补丁源码 (编译时内嵌进 Rust 二进制)
│   ├── futubull_patch.m        # 针对新版 Futubull (FTNN) 的补丁 (全中文大白话注释)
│   └── futuniuniu_patch.m      # 针对旧版 FutuNiuniu (legacy) 的补丁 (全中文大白话注释，彻底移除 .real)
└── src/
    ├── main.rs                 # CLI 入口：交互式终端菜单与命令行参数解析
    ├── macho.rs                # 纯 Rust 实现的 Mach-O LC_LOAD_DYLIB 尾部无损注入器 (带填充区零值防护)
    ├── patcher.rs              # 核心执行流水线：自动终止进程 -> 恢复 .real -> 编译 -> 注入 -> 瘦身 -> 签名
    └── cleaner.rs              # 冗余组件清理、CEF 语言包清理、SUDO_USER 真实目录清理模块
```

---

### 六、 免责声明

本项目仅供个人学习、系统清理与逆向工程学术研究使用。所有修改均在本地用户主机上完成，不含任何商业用途。
