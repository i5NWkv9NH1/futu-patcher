// * futu-patcher 主入口：命令行解析、交互式菜单与任务调度

pub mod cleaner;
pub mod macho;
pub mod patcher;

use clap::{Parser, ValueEnum};
use console::{Emoji, style};
use dialoguer::{MultiSelect, Select};
use patcher::{FutuApp, PatchOptions, PatchReport, run_documents_clean, run_pipeline_for_app};
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

static PACKAGE_EMOJI: Emoji<'_, '_> = Emoji("📦 ", "");
static ROCKET_EMOJI: Emoji<'_, '_> = Emoji("🚀 ", "");
static SPARKLE_EMOJI: Emoji<'_, '_> = Emoji("✨ ", "");
static CHART_EMOJI: Emoji<'_, '_> = Emoji("📊 ", "");

// * 命令行可选的目标富途客户端
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
    about = "macOS 富途客户端 (Futubull / FutuNiuniu) 净化与瘦身工具",
    long_about = "一站式解决富途客户端后台冗余上报、IMSDK 垃圾污染、体积庞大与退出卡死问题。\n支持新版 Futubull (Qt+CEF) 与旧版 FutuNiuniu (Cocoa legacy)。"
)]
pub struct CliArgs {
    // * 直接使用默认配置全自动运行（非交互静默模式）
    #[arg(
        short = 'y',
        long = "yes",
        help = "跳过交互确认，以默认全选配置一键执行所有已安装客户端"
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

    // * 单独开关：禁用核心注入补丁
    #[arg(long = "no-core", help = "跳过核心注入补丁 (保留原版网络行为与路径)")]
    pub no_core: bool,

    // * 单独开关：跳过冗余组件清理
    #[arg(
        long = "no-bloat",
        help = "跳过冗余组件清理 (保留 CrashReporter、Python 等)"
    )]
    pub no_bloat: bool,

    // * 单独开关：跳过 CEF 语言包与快照精简
    #[arg(long = "no-locales", help = "跳过精简语言包")]
    pub no_locales: bool,

    // * 单独开关：跳过 lipo arm64 架构提取
    #[arg(long = "no-thin", help = "跳过 lipo thin arm64 通用二进制剥离")]
    pub no_thin: bool,

    // * 单独开关：跳过 ~/Documents 历史缓存清理
    #[arg(long = "no-docs", help = "跳过 ~/Documents 历史残留目录清理")]
    pub no_docs: bool,

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
fn init_tracing(verbose: bool) {
    let level = if verbose { Level::DEBUG } else { Level::INFO };
    let subscriber = FmtSubscriber::builder()
        .with_max_level(level)
        .with_target(false)
        .without_time()
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
}

// * 检查是否具备写入 /Applications 的管理员权限
fn check_root_privilege() {
    let is_root = unsafe { libc::geteuid() == 0 };
    if !is_root {
        eprintln!(
            "{}",
            style("⚠️  警告: 本工具需要修改 /Applications 下的富途 App，建议使用 sudo futu-patcher 运行！")
                .yellow()
                .bold()
        );
    }
}

// * 打印优雅的汇总报表
fn print_summary_report(reports: &[PatchReport]) {
    println!();
    println!(
        "{}",
        style("==========================================================").dim()
    );
    println!(
        "{} {}",
        CHART_EMOJI,
        style("净化与瘦身汇总统计").cyan().bold()
    );
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
            "  体积对比: {} -> {} (瘦身: {}, 减幅: {:.1}%)",
            cleaner::format_size(r.initial_size),
            cleaner::format_size(r.final_size),
            cleaner::format_size(saved),
            percent
        );

        if r.restored_real {
            println!(
                "  历史修复: {}",
                style("已自动还原残留 .real 二进制").yellow()
            );
        }

        if let Some(st) = r.inject_status {
            let desc = match st {
                macho::InjectResult::Injected => "成功追加至加载表末尾",
                macho::InjectResult::AlreadyPresent => "已就绪 (无需重复写入)",
                macho::InjectResult::FixedAndInjected => "修复错位项并重写至末尾",
            };
            println!("  核心补丁: {}", style(desc).cyan());
        }

        if !r.removed_components.is_empty() {
            println!("  移除组件: {} 个", r.removed_components.len());
        }

        if !r.pruned_locales.is_empty() {
            println!("  精简语言包: {} 个", r.pruned_locales.len());
        }

        if r.thinned_count > 0 {
            println!(
                "  架构瘦身: {} 个二进制文件完成纯 arm64 剥离",
                r.thinned_count
            );
        }

        println!();
    }

    println!(
        "{} 累计释放磁盘空间: {}",
        SPARKLE_EMOJI,
        style(cleaner::format_size(total_saved)).yellow().bold()
    );
    println!(
        "{}",
        style("----------------------------------------------------------").dim()
    );
    println!("💡 代理分流提示: 进程名已维持官方原生，Surge / Clash 请使用以下规则:");
    println!("   {}", style("PROCESS-NAME,FTNN,DIRECT").dim());
    println!("   {}", style("PROCESS-NAME,FutuNiuniu,DIRECT").dim());
    println!(
        "{}",
        style("==========================================================").dim()
    );
}

fn main() {
    let args = CliArgs::parse();
    init_tracing(args.verbose);

    println!(
        "{} {}",
        PACKAGE_EMOJI,
        style("富途客户端 (Futubull / FutuNiuniu) 净化与瘦身工具 v0.1.0")
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
            style("❌ 未在 /Applications 目录下检测到任何富途客户端！")
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
            "1. 核心注入补丁 (IMSDK 沙盒隔离 + 网络双重网闸 + 内存防上报 + Cmd+Q 秒退)",
            "2. 移除冗余组件 (CrashReporter、NNPython、AutoTest、声卡驱动)",
            "3. 精简语言包与快照 (CEF 非必要语言包与 x86_64 快照)",
            "4. 提取纯 arm64 架构 (通过 lipo 剥离 x86_64 架构瘦身)",
            "5. 清理历史残留目录 (~/Documents 遗留缓存清理)",
        ];

        let defaults = [true, true, true, true, true];

        println!();
        println!("请选择需要执行的净化与精简项目 (空格切换，回车确认):");
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
                // * 默认全选处理本机存在的所有客户端
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

    println!();
    println!("{} 正在启动净化与瘦身流水线...", ROCKET_EMOJI);

    let mut reports = Vec::new();

    for app in &targets {
        match run_pipeline_for_app(*app, &options) {
            Ok(report) => reports.push(report),
            Err(e) => {
                eprintln!(
                    "{}",
                    style(format!("❌ 处理客户端「{}」失败: {e}", app.display_name()))
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
