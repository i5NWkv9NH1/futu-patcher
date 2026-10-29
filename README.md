# futu-patcher: macOS 富途客户端补丁与精简工具

[简体中文](./README.md) | [English](./README_EN.md)

> [!NOTE]
> 本项目为个人自用工具，由 **Gemini 3.8 Flash** 与 **Gemini 4 Argon** 协助生成。

基于 **Rust 2024 Edition** 构建的 macOS 富途客户端补丁与精简工具。解决富途后台频繁埋点、IMSDK 垃圾文件污染 `~/Documents`、安装包体积冗余以及 `Cmd+Q` 退出卡死问题。

---

### 一、 客户端对比

| 特性维度 | Futubull (富途牛牛新版) | FutuNiuniu (富途牛牛经典版) |
| :--- | :--- | :--- |
| **安装路径** | `/Applications/Futubull.app` | `/Applications/FutuNiuniu.app` |
| **主二进制** | `FTNN` | `FutuNiuniu` |
| **技术栈** | Qt 5/6 + CEF (Chromium) | 原生 macOS Cocoa (AppKit) |
| **维护状态** | 官方持续维护 | 官方已停更 (`brew install --cask futubull@legacy`) |
| **注入方式** | 纯 Rust Mach-O 尾部 `LC_LOAD_DYLIB` | 纯 Rust Mach-O 尾部 `LC_LOAD_DYLIB` |
| **拦截策略** | 屏蔽 `F3CReport`、`libFTApmKit` 与 `F3CLogUploader` | 屏蔽 `F3CReport` 与 `Firebase` |
| **实测节省空间** | 约 **918 MB** (`1.84 GB -> 966 MB`，减少约 `48.7%`) | 约 **178 MB** (`457 MB -> 279 MB`，减少约 `39.0%`) |

---

### 二、 核心功能

1. **注入补丁**：将 `com_tencent_imsdk_data` 与 `LocalizedString` 重定向至沙盒目录，拦截 DNS 与 IP 埋点，内存屏蔽上报逻辑，400 毫秒延迟退出机制解决 `Cmd+Q` 卡死。
2. **删除无用组件**：删除 `CrashReporter.app`、`NNPython.app`、`libFTAutoTest.dylib` 与虚拟声卡驱动。
3. **删除多余语言包与快照**：删除 CEF 冗余多国语言包与 x86_64 架构 V8 快照。
4. **提取 arm64 架构**：调用 `lipo -thin arm64` 剥离通用二进制中的 `x86_64` 代码。
5. **清理残留文件夹**：读取 `SUDO_USER` 真实主目录，清理 `~/Documents` 下的遗留缓存。

---

### 三、 编译与运行

#### 方式一：一行命令直接运行（无需安装 Rust 环境）

```bash
# 交互式运行 (默认全选)
curl -fsSL https://raw.githubusercontent.com/i5NWkv9NH1/futu-patcher/master/install.sh | bash

# 一键静默执行全部优化
curl -fsSL https://raw.githubusercontent.com/i5NWkv9NH1/futu-patcher/master/install.sh | bash -s -- -y
```

#### 方式二：从源码编译运行

```bash
# 编译发布版本
cargo build --release

# 交互模式运行 (默认全选)
sudo ./target/release/futu-patcher

# 一键静默全自动处理已安装客户端
sudo ./target/release/futu-patcher -y

# 诊断模式：探测 Futubull 客户端结构与导出符号 (无需管理员权限)
./target/release/futu-patcher -i
```

---

### 四、 代理分流规则

客户端打补丁后维持官方原生进程名不变，Surge / Clash 分流规则直接配置如下：

```text
PROCESS-NAME,FTNN,DIRECT
PROCESS-NAME,FutuNiuniu,DIRECT
```

---

### 五、 后续版本更新维护说明

当富途牛牛新版更新后，可通过诊断模式辅助定位变动：

1. **探测新版结构**：运行 `./target/release/futu-patcher -i`，工具会自动读取版本号、统计包内组件体积、导出 `FTNN` 依赖列表，并扫描 `F3CReport`、`libFTApmKit` 与 `F3CLogUploader` 内部的上报与监控符号。
2. **核对符号变动**：若富途调整了埋点函数命名，将诊断输出中的新符号同步更新至 `patches/futu_hook.rs` 的 `patch_f3c_report` 或 `patch_ft_apm` 逻辑中。
3. **重新打补丁**：运行 `sudo futu-patcher` 即可自动完成编译与注入。
