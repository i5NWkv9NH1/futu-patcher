// * futu-patcher 核心库：模块导出、CLI 解析与调度

pub mod cleaner;
pub mod macho;
pub mod patcher;

use clap::{Parser, ValueEnum};
use console::style;
use dialoguer::{MultiSelect, Select};
use patcher::{FutuApp, PatchOptions, PatchReport, run_documents_clean, run_pipeline_for_app};
use std::path::PathBuf;
use std::process::Command;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

// * 命令行可选的目标客户端
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TargetArg {
    #[value(name = "futubull")]
    Futubull,
    #[value(name = "futuniuniu")]
    FutuNiuniu,
    #[value(name = "both")]
    Both,
}

// * CLI 参数定义结构体
#[derive(Parser, Debug)]
#[command(
    name = "futu-patcher",
    author = "sora",
    version,
    about = "macOS 富途客户端 (Futubull / FutuNiuniu) 补丁与精简工具",
    long_about = "解决富途客户端后台冗余上报、IMSDK 垃圾污染、体积庞大与退出卡死问题。\n支持 Futubull (富途牛牛新版) 与 FutuNiuniu (富途牛牛经典版)。"
)]
pub struct CliArgs {
    // * 直接使用默认配置全自动运行（非交互静默模式）
    #[arg(
        short = 'y',
        long = "yes",
        help = "跳过交互确认，以默认配置执行所有已安装客户端"
    )]
    pub yes: bool,

    // * 指定目标客户端
    #[arg(
        short = 't',
        long = "target",
        value_enum,
        help = "指定处理客户端: futubull, futuniuniu 或 both"
    )]
    pub target: Option<TargetArg>,

    // * 单独开关：禁用注入补丁
    #[arg(long = "no-core", help = "跳过注入补丁 (保留原版网络行为与路径)")]
    pub no_core: bool,

    // * 单独开关：跳过无用组件清理
    #[arg(
        long = "no-bloat",
        help = "跳过删除无用组件 (保留 CrashReporter、Python 等)"
    )]
    pub no_bloat: bool,

    // * 单独开关：跳过语言包与快照精简
    #[arg(long = "no-locales", help = "跳过删除多余语言包与快照")]
    pub no_locales: bool,

    // * 单独开关：跳过 lipo arm64 架构提取
    #[arg(long = "no-thin", help = "跳过 lipo thin arm64 通用二进制剥离")]
    pub no_thin: bool,

    // * 单独开关：跳过 ~/Documents 历史缓存清理
    #[arg(long = "no-docs", help = "跳过 ~/Documents 残留文件夹清理")]
    pub no_docs: bool,

    // * 诊断模式：探测客户端结构与符号
    #[arg(
        short = 'i',
        long = "inspect",
        help = "诊断模式：探测 Futubull 客户端结构与导出符号"
    )]
    pub inspect: bool,

    // * 开启详细调试日志
    #[arg(short = 'v', long = "verbose", help = "输出底层详细执行日志")]
    pub verbose: bool,
}

impl CliArgs {
    // * 根据 CLI 参数生成 PatchOptions
    pub fn to_patch_options(&self) -> PatchOptions {
        PatchOptions {
            core_patch: !self.no_core,
            remove_bloatware: !self.no_bloat,
            prune_locales: !self.no_locales,
            thin_arm64: !self.no_thin,
            clean_documents: !self.no_docs,
        }
    }
}

// * 初始化日志输出格式
pub fn init_tracing(verbose: bool) {
    let level = if verbose { Level::DEBUG } else { Level::INFO };
    let subscriber = FmtSubscriber::builder()
        .with_max_level(level)
        .with_target(false)
        .without_time()
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
}

// * 检查是否具备写入 /Applications 的管理员权限
pub fn check_root_privilege() {
    let is_root = unsafe { libc::geteuid() == 0 };
    if !is_root {
        eprintln!(
            "{}",
            style("提示: 修改 /Applications 下的富途客户端需要管理员权限，建议使用 sudo futu-patcher 运行！")
                .yellow()
                .bold()
        );
    }
}

// * 打印汇总报表
pub fn print_summary_report(reports: &[PatchReport]) {
    println!();
    println!(
        "{}",
        style("==========================================================").dim()
    );
    println!("{}", style("处理汇总统计").cyan().bold());
    println!(
        "{}",
        style("----------------------------------------------------------").dim()
    );

    let mut total_saved = 0u64;

    for r in reports {
        let saved = r.initial_size.saturating_sub(r.final_size);
        total_saved += saved;
        let percent = if r.initial_size > 0 {
            (saved as f64 / r.initial_size as f64) * 100.0
        } else {
            0.0
        };

        println!("• 目标客户端: {}", style(&r.app_name).green().bold());
        println!(
            "  体积对比: {} -> {} (减少: {}, 减幅: {:.1}%)",
            cleaner::format_size(r.initial_size),
            cleaner::format_size(r.final_size),
            cleaner::format_size(saved),
            percent
        );

        if let Some(st) = r.inject_status {
            let desc = match st {
                macho::InjectResult::Injected => "成功追加至加载表末尾",
                macho::InjectResult::AlreadyPresent => "已就绪 (无需重复写入)",
                macho::InjectResult::FixedAndInjected => "修复错位项并重写至末尾",
            };
            println!("  补丁状态: {}", style(desc).cyan());
        }

        if !r.removed_components.is_empty() {
            println!("  删除组件: {} 个", r.removed_components.len());
        }

        if !r.pruned_locales.is_empty() {
            println!("  精简语言包: {} 个", r.pruned_locales.len());
        }

        if r.thinned_count > 0 {
            println!("  架构精简: {} 个二进制完成纯 arm64 转换", r.thinned_count);
        }

        println!();
    }

    println!(
        "累计释放磁盘空间: {}",
        style(cleaner::format_size(total_saved)).yellow().bold()
    );
    println!(
        "{}",
        style("----------------------------------------------------------").dim()
    );
    println!("代理分流规则 (Surge / Clash):");
    println!("  {}", style("PROCESS-NAME,FTNN,DIRECT").dim());
    println!("  {}", style("PROCESS-NAME,FutuNiuniu,DIRECT").dim());
    println!(
        "{}",
        style("==========================================================").dim()
    );
}

// * 诊断模式：探测 Futubull 客户端结构与导出符号
pub fn run_inspect() {
    let app_path = PathBuf::from("/Applications/Futubull.app");
    if !app_path.exists() {
        eprintln!(
            "{}",
            style("未在系统中检测到 /Applications/Futubull.app")
                .red()
                .bold()
        );
        return;
    }

    println!(
        "{}",
        style("==========================================================").dim()
    );
    println!(
        "{}",
        style("Futubull 客户端结构与符号诊断报告").cyan().bold()
    );
    println!(
        "{}",
        style("----------------------------------------------------------").dim()
    );

    // * 1. 读取版本号
    let plist_path = app_path.join("Contents/Info.plist");
    let version_cmd = Command::new("/usr/libexec/PlistBuddy")
        .arg("-c")
        .arg("Print :CFBundleShortVersionString")
        .arg(&plist_path)
        .output();
    let build_cmd = Command::new("/usr/libexec/PlistBuddy")
        .arg("-c")
        .arg("Print :CFBundleVersion")
        .arg(&plist_path)
        .output();

    let version_str = version_cmd
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "未知".to_string());

    let build_str = build_cmd
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "未知".to_string());

    println!(
        "客户端版本: {} (Build {})",
        style(&version_str).green().bold(),
        build_str
    );
    println!("应用路径: {}", app_path.display());
    println!();

    // * 2. 扫描组件
    println!("{}", style("主要组件列表 (按体积降序):").yellow().bold());
    let scan_dirs = [
        app_path.join("Contents/Frameworks"),
        app_path.join("Contents/PlugIns"),
        app_path.join("Contents/MacOS"),
    ];

    let mut components: Vec<(String, u64)> = Vec::new();
    for dir in &scan_dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".app")
                    || name.ends_with(".framework")
                    || name.ends_with(".dylib")
                    || name.ends_with(".driver")
                    || name.ends_with(".plugin")
                {
                    let size = if path.is_dir() {
                        cleaner::calculate_dir_size(&path)
                    } else {
                        entry.metadata().map(|m| m.len()).unwrap_or(0)
                    };
                    components.push((name, size));
                }
            }
        }
    }

    components.sort_by_key(|a| std::cmp::Reverse(a.1));
    for (name, size) in &components {
        println!(
            "  - {:<48} {}",
            name,
            style(cleaner::format_size(*size)).dim()
        );
    }
    println!();

    // * 3. 动态库加载表 (otool -L FTNN)
    println!("{}", style("主二进制 (FTNN) 动态库依赖表:").yellow().bold());
    let main_bin = app_path.join("Contents/MacOS/FTNN");
    let otool_res = Command::new("/usr/bin/otool")
        .arg("-L")
        .arg(&main_bin)
        .output();
    if let Ok(out) = otool_res {
        let stdout = String::from_utf8_lossy(&out.stdout);
        for line in stdout.lines().skip(1) {
            println!("  {}", line.trim());
        }
    }
    println!();

    // * 4. 符号扫描 (nm -gU)
    println!(
        "{}",
        style("上报相关敏感符号扫描 (F3CReport / libFTApmKit):")
            .yellow()
            .bold()
    );
    let symbol_targets = [
        (
            "F3CReport",
            app_path.join("Contents/Frameworks/F3CReport.framework/Versions/Current/F3CReport"),
        ),
        (
            "libFTApmKit.dylib",
            app_path.join("Contents/Frameworks/libFTApmKit.dylib"),
        ),
    ];

    let keywords = ["Report", "Upload", "Init", "Flush", "Apm", "Crash", "Jank"];

    for (target_name, bin_path) in &symbol_targets {
        if !bin_path.exists() {
            continue;
        }
        println!("  • [{}]:", target_name);
        let nm_res = Command::new("/usr/bin/nm")
            .arg("-gU")
            .arg(bin_path)
            .output();
        if let Ok(out) = nm_res {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let mut matched_count = 0;
            for line in stdout.lines() {
                if keywords.iter().any(|&k| line.contains(k)) {
                    let sym = line.split_whitespace().last().unwrap_or(line);
                    println!("    {}", sym);
                    matched_count += 1;
                }
            }
            if matched_count == 0 {
                println!("    (未检测到匹配符号)");
            }
        }
    }
    println!(
        "{}",
        style("==========================================================").dim()
    );
}

// * 主流程执行入口
pub fn run() {
    let args = CliArgs::parse();
    init_tracing(args.verbose);

    // * 若开启诊断探测模式，执行后直接退出
    if args.inspect {
        run_inspect();
        return;
    }

    println!(
        "{}",
        style("富途客户端 (Futubull / FutuNiuniu) 补丁与精简工具 v0.1.0")
            .bold()
            .cyan()
    );
    check_root_privilege();

    // * 1. 扫描本机已安装的客户端
    let futubull = FutuApp::Futubull;
    let futuniuniu = FutuApp::FutuNiuniu;

    let futubull_installed = futubull.is_installed();
    let futuniuniu_installed = futuniuniu.is_installed();

    if !futubull_installed && !futuniuniu_installed {
        eprintln!(
            "{}",
            style("未在 /Applications 目录下检测到任何富途客户端！")
                .red()
                .bold()
        );
        eprintln!(
            "   请确认是否已安装「/Applications/Futubull.app」或「/Applications/FutuNiuniu.app」。"
        );
        std::process::exit(1);
    }

    // * 2. 确定目标客户端与执行选项
    let is_interactive = !args.yes && args.target.is_none() && console::user_attended();

    let (targets, options) = if is_interactive {
        // * 交互模式：让用户选择目标客户端
        let mut app_choices: Vec<(String, Vec<FutuApp>)> = Vec::new();
        if futubull_installed && futuniuniu_installed {
            app_choices.push(("Both (全部处理)".to_string(), vec![futubull, futuniuniu]));
        }
        if futubull_installed {
            app_choices.push((futubull.display_name().to_string(), vec![futubull]));
        }
        if futuniuniu_installed {
            app_choices.push((futuniuniu.display_name().to_string(), vec![futuniuniu]));
        }

        let selection_names: Vec<&str> = app_choices.iter().map(|(n, _)| n.as_str()).collect();

        let selected_idx = Select::new()
            .with_prompt("请选择要处理的客户端")
            .items(&selection_names)
            .default(0)
            .interact()
            .unwrap_or(0);

        let chosen_apps = app_choices[selected_idx].1.clone();

        // * 交互模式：多选菜单（默认全选）
        let module_items = [
            "1. 注入补丁 (隔离文档目录 / 屏蔽埋点请求 / 修复 Cmd+Q 退出卡死)",
            "2. 删除无用组件 (CrashReporter / NNPython / AutoTest / 虚拟声卡)",
            "3. 删除多余语言包与 x86 快照",
            "4. 提取 arm64 架构 (lipo 移除 x86_64 代码)",
            "5. 清理 ~/Documents 残留文件夹",
        ];

        let defaults = [true, true, true, true, true];

        println!();
        println!("请选择需要执行的项目 (空格切换，回车确认):");
        let selections = MultiSelect::new()
            .items(&module_items)
            .defaults(&defaults)
            .interact()
            .unwrap_or_else(|_| vec![0, 1, 2, 3, 4]);

        let chosen_options = PatchOptions {
            core_patch: selections.contains(&0),
            remove_bloatware: selections.contains(&1),
            prune_locales: selections.contains(&2),
            thin_arm64: selections.contains(&3),
            clean_documents: selections.contains(&4),
        };

        (chosen_apps, chosen_options)
    } else {
        // * 命令行非交互模式
        let target_apps = match args.target {
            Some(TargetArg::Futubull) => vec![futubull],
            Some(TargetArg::FutuNiuniu) => vec![futuniuniu],
            Some(TargetArg::Both) => {
                let mut v = Vec::new();
                if futubull_installed {
                    v.push(futubull);
                }
                if futuniuniu_installed {
                    v.push(futuniuniu);
                }
                v
            }
            None => {
                let mut v = Vec::new();
                if futubull_installed {
                    v.push(futubull);
                }
                if futuniuniu_installed {
                    v.push(futuniuniu);
                }
                v
            }
        };

        (target_apps, args.to_patch_options())
    };

    let mut reports = Vec::new();

    for app in &targets {
        match run_pipeline_for_app(*app, &options) {
            Ok(report) => reports.push(report),
            Err(e) => {
                eprintln!(
                    "{}",
                    style(format!("处理客户端「{}」失败: {e}", app.display_name()))
                        .red()
                        .bold()
                );
            }
        }
    }

    if options.clean_documents {
        run_documents_clean();
    }

    if !reports.is_empty() {
        print_summary_report(&reports);
    }
}
