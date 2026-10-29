# futu-patcher: macOS 富途客户端净化与瘦身工具

[简体中文](./README.md) | [English](./README_EN.md)

> [!NOTE]
> 本项目为个人自用工具，由 **Gemini 3.8 Flash** 与 **Gemini 4 Argon** 协助生成。

基于 **Rust 2024 Edition** 构建的 macOS 富途客户端补丁与瘦身工具。解决富途后台频繁埋点、IMSDK 垃圾文件污染 `~/Documents`、安装包体积冗余以及 `Cmd+Q` 退出卡死问题。

---

### 一、 客户端对比

| 特性维度 | Futubull (新版长期维护版) | FutuNiuniu (旧版 legacy 原生版) |
| :--- | :--- | :--- |
| **安装路径** | `/Applications/Futubull.app` | `/Applications/FutuNiuniu.app` |
| **主二进制** | `FTNN` | `FutuNiuniu` |
| **技术栈** | Qt 5/6 + CEF (Chromium) | 原生 macOS Cocoa (AppKit) |
| **维护状态** | 官方持续维护 | 官方已停更 (`brew install --cask futubull@legacy`) |
| **注入方式** | 纯 Rust Mach-O 尾部 `LC_LOAD_DYLIB` | 纯 Rust Mach-O 尾部 `LC_LOAD_DYLIB` (废除 `.real`) |
| **拦截策略** | 屏蔽 `F3CReport` 与 `libFTApmKit` | 屏蔽 `F3CReport` 与 `Firebase` |
| **瘦身收益** | **200MB ~ 400MB+** | **50MB ~ 100MB+** |

---

### 二、 核心功能

1. **注入补丁**：将 `com_tencent_imsdk_data` 与 `LocalizedString` 重定向至沙盒目录，拦截 DNS 与 IP 埋点，内存屏蔽上报逻辑，400ms 看门狗解决 `Cmd+Q` 卡死。
2. **删除无用组件**：删除 `CrashReporter.app`、`NNPython.app`、`libFTAutoTest.dylib` 与虚拟声卡驱动。
3. **删除多余语言包与快照**：删除 CEF 冗余多国语言包与 x86_64 架构 V8 快照。
4. **提取 arm64 架构**：调用 `lipo -thin arm64` 剥离通用二进制中的 `x86_64` 代码。
5. **清理残留文件夹**：读取 `SUDO_USER` 真实主目录，清理 `~/Documents` 下的遗留缓存。

---

### 三、 编译与运行

```bash
# 编译发布版本
cargo build --release

# 交互模式运行 (默认全选)
sudo ./target/release/futu-patcher

# 一键静默全自动处理已安装客户端
sudo ./target/release/futu-patcher -y
```

---

### 四、 代理分流规则

客户端打补丁后维持官方原生进程名不变，Surge / Clash 分流规则直接配置如下：

```text
PROCESS-NAME,FTNN,DIRECT
PROCESS-NAME,FutuNiuniu,DIRECT
```
