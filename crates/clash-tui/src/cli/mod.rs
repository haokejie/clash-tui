use std::{io::Read as _, path::PathBuf, sync::Arc, time::Duration};

use anyhow::Result;
use clap::{
    Arg, ArgAction, Args, Command as ClapCommand, CommandFactory as _, FromArgMatches as _, Parser, Subcommand,
    ValueEnum,
};
use clash_core::{IProfiles, LocalProfileImport, PrfItem, RemoteProfileImport, config::profiles::generate_remote_uid};
use serde::Serialize;
use serde_json::json;

use crate::{
    actions,
    jobs::{JobRecord, JobStatus},
    mihomo_controller::{Mode, RulesResponse},
    options::{
        ClashTuiOptions, DEFAULT_SUBSCRIPTION_CHECK_INTERVAL_SECS, ENV_HOME_DIR, ENV_MIHOMO_BIN, ENV_RESOURCE_DIR,
        ENV_SUBSCRIPTION_CHECK_INTERVAL_SECS,
    },
    state::AppState,
};

#[derive(Debug, Parser)]
#[command(
    version = env!("CLASH_TUI_APP_VERSION"),
    about = "mihomo 本地 TUI/CLI 控制器",
    long_about = "默认不带子命令会进入中文 TUI。CLI 子命令用于脚本化管理 Core、订阅、代理、规则、连接、任务、TUN 和系统代理。",
    subcommand_help_heading = "命令",
    next_help_heading = "全局选项",
    disable_help_flag = true,
    disable_version_flag = true,
    disable_help_subcommand = true,
    help_template = "{before-help}{name} {version}\n{about-with-newline}\n用法：{usage}\n\n{all-args}{after-help}",
    after_help = "示例：\n  clash-tui\n  clash-tui profile import-url --stdin --start-core\n  clash-tui --json proxy groups"
)]
pub struct Cli {
    #[arg(short = 'h', long = "help", global = true, action = ArgAction::Help, help = "显示帮助信息")]
    pub help: Option<bool>,
    #[arg(short = 'V', long = "version", action = ArgAction::Version, help = "显示版本信息")]
    pub version: Option<bool>,
    #[arg(long, env = ENV_HOME_DIR, global = true, help = "指定应用 home 目录")]
    pub home_dir: Option<PathBuf>,
    #[arg(long, env = ENV_RESOURCE_DIR, global = true, help = "指定资源目录")]
    pub resource_dir: Option<PathBuf>,
    #[arg(long, env = ENV_MIHOMO_BIN, global = true, help = "指定 mihomo 可执行文件路径")]
    pub mihomo_bin: Option<PathBuf>,
    #[arg(
        long,
        env = ENV_SUBSCRIPTION_CHECK_INTERVAL_SECS,
        default_value_t = DEFAULT_SUBSCRIPTION_CHECK_INTERVAL_SECS,
        global = true,
        help = "兼容保留：不启动后台定时器，订阅到期检查仅在 TUI 启动或手动 --due 时执行"
    )]
    pub subscription_check_interval_secs: u64,
    #[arg(long, global = true, help = "以 JSON 输出结果，适合脚本调用")]
    pub json: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(about = "打开中文 TUI 控制台")]
    Tui,
    #[command(about = "启动、停止、重启和查看 mihomo Core")]
    Core {
        #[command(subcommand)]
        command: CoreCommand,
    },
    #[command(about = "查看或切换代理模式")]
    Mode {
        #[command(subcommand)]
        command: ModeCommand,
    },
    #[command(about = "管理 Profile 和订阅导入")]
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    #[command(about = "查看策略组并选择节点")]
    Proxy {
        #[command(subcommand)]
        command: ProxyCommand,
    },
    #[command(about = "查看或修改常用设置")]
    Settings {
        #[command(subcommand)]
        command: SettingsCommand,
    },
    #[command(about = "生成、查看和校验 runtime 配置")]
    Runtime {
        #[command(subcommand)]
        command: RuntimeCommand,
    },
    #[command(about = "查看或搜索规则列表")]
    Rules {
        #[command(subcommand)]
        command: RulesCommand,
    },
    #[command(about = "查看或关闭连接")]
    Connections {
        #[command(subcommand)]
        command: ConnectionsCommand,
    },
    #[command(about = "更新或测速 Provider")]
    Provider {
        #[command(subcommand)]
        command: ProviderCommand,
    },
    #[command(about = "更新订阅并查看启动检查状态")]
    Subscription {
        #[command(subcommand)]
        command: SubscriptionCommand,
    },
    #[command(about = "查看、重试或取消后台任务")]
    Jobs {
        #[command(subcommand)]
        command: JobsCommand,
    },
    #[command(about = "生成脱敏诊断报告")]
    Diagnose(DiagnoseArgs),
    #[command(
        about = "开启、关闭、查看或诊断 TUN",
        long_about = "开启、关闭、查看或诊断 TUN。Linux 通常需要 /dev/net/tun，并以 root 运行或为 mihomo 授予 CAP_NET_ADMIN；非 root 排查建议准备 getcap/setcap。开启后会改写 runtime 并在 Core 启动时创建 TUN 网卡和路由。操作前建议先执行 tun doctor；若网络异常，执行 tun off 后再执行 core stop 恢复。doctor 只读检查当前环境，不修改配置、不创建网卡、不改路由。"
    )]
    Tun {
        #[command(subcommand)]
        command: SwitchCommand,
    },
    #[command(
        name = "system-proxy",
        about = "开启、关闭、查看或诊断系统代理",
        long_about = "开启、关闭、查看或诊断系统代理。开启会尝试修改桌面系统代理；Linux 自动应用依赖 GNOME gsettings 的 org.gnome.system.proxy schema。不支持时会回滚配置，并在 JSON manualAction 中给出手动设置建议。操作前建议先执行 system-proxy doctor；若桌面代理异常，执行 system-proxy off 恢复。doctor 只读检查当前环境，不修改系统代理。"
    )]
    SystemProxy {
        #[command(subcommand)]
        command: SystemProxyCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum CoreCommand {
    #[command(about = "启动 Core")]
    Start,
    #[command(about = "停止 Core")]
    Stop,
    #[command(about = "重启 Core")]
    Restart,
    #[command(about = "以前台方式运行 Core（供 systemd/supervisor 托管）")]
    Run,
    #[command(about = "查看 Core 状态")]
    Status,
    #[command(about = "查看 Core 日志摘要")]
    Logs,
}

#[derive(Debug, Subcommand)]
pub enum ModeCommand {
    #[command(about = "查看当前代理模式")]
    Get,
    #[command(about = "切换代理模式")]
    Set {
        #[arg(help = "代理模式：rule/global/direct")]
        mode: ModeArg,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ModeArg {
    Rule,
    Global,
    Direct,
}

impl From<ModeArg> for Mode {
    fn from(value: ModeArg) -> Self {
        match value {
            ModeArg::Rule => Self::Rule,
            ModeArg::Global => Self::Global,
            ModeArg::Direct => Self::Direct,
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum ProfileCommand {
    #[command(about = "列出 Profile")]
    List,
    #[command(about = "查看当前 Profile")]
    Current,
    #[command(about = "切换 Profile")]
    Switch {
        #[arg(help = "Profile ID")]
        id: String,
    },
    #[command(about = "导入本地 YAML 配置")]
    ImportLocal(ImportLocalArgs),
    #[command(about = "导入订阅链接，建议配合 --stdin 避免 shell history 泄露")]
    ImportUrl(ImportUrlArgs),
}

#[derive(Debug, Args)]
pub struct ImportLocalArgs {
    #[arg(long, help = "指定导入后的 Profile ID")]
    pub id: Option<String>,
    #[arg(long, help = "指定显示名称")]
    pub name: Option<String>,
    #[arg(help = "本地 YAML 配置文件路径")]
    pub file: PathBuf,
}

#[derive(Debug, Args)]
pub struct ImportUrlArgs {
    #[arg(long, help = "指定导入后的 Profile ID")]
    pub id: Option<String>,
    #[arg(long, help = "指定显示名称")]
    pub name: Option<String>,
    #[arg(long, help = "导入后立即激活")]
    pub activate: bool,
    #[arg(long, help = "导入后激活并启动 Core")]
    pub start_core: bool,
    #[arg(
        long,
        conflicts_with = "url",
        required_unless_present = "url",
        help = "从标准输入读取订阅链接"
    )]
    pub stdin: bool,
    #[arg(required_unless_present = "stdin", help = "订阅链接")]
    pub url: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum ProxyCommand {
    #[command(about = "列出策略组和节点")]
    Groups,
    #[command(about = "为策略组选择节点")]
    Select {
        #[arg(help = "策略组名称")]
        group: String,
        #[arg(help = "节点名称")]
        proxy: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum SettingsCommand {
    #[command(about = "显示当前设置")]
    Show,
    #[command(about = "修改设置项")]
    Set {
        #[arg(help = "设置项")]
        key: SettingsKeyArg,
        #[arg(help = "设置值")]
        value: String,
    },
    #[command(about = "管理 DNS 覆写配置")]
    Dns {
        #[command(subcommand)]
        command: DnsCommand,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SettingsKeyArg {
    Dns,
    Ipv6,
    #[value(name = "allow-lan")]
    AllowLan,
    #[value(name = "unified-delay")]
    UnifiedDelay,
    #[value(name = "log-level")]
    LogLevel,
    #[value(name = "core-log")]
    CoreLog,
    #[value(name = "mixed-port")]
    MixedPort,
    #[value(name = "external-controller")]
    ExternalController,
    #[value(name = "external-controller-port")]
    ExternalControllerPort,
}

#[derive(Debug, Subcommand)]
pub enum DnsCommand {
    #[command(about = "显示 DNS 覆写配置")]
    Show,
    #[command(about = "保存 DNS 覆写配置")]
    Save {
        #[arg(help = "DNS YAML 文件路径")]
        file: PathBuf,
    },
    #[command(about = "校验 DNS 覆写配置")]
    Validate,
}

#[derive(Debug, Subcommand)]
pub enum RuntimeCommand {
    #[command(about = "生成 runtime 配置")]
    Generate,
    #[command(about = "显示 runtime YAML")]
    Show,
    #[command(about = "校验 runtime 配置")]
    Validate,
}

#[derive(Debug, Subcommand)]
pub enum RulesCommand {
    #[command(about = "列出规则")]
    List {
        #[arg(long, help = "限制返回条数")]
        limit: Option<usize>,
    },
    #[command(about = "搜索规则")]
    Search {
        #[arg(help = "搜索关键字")]
        query: String,
        #[arg(long, help = "限制返回条数")]
        limit: Option<usize>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConnectionsCommand {
    #[command(about = "列出连接")]
    List,
    #[command(about = "关闭指定连接")]
    Close {
        #[arg(help = "连接 ID")]
        id: String,
    },
    #[command(name = "close-all")]
    #[command(about = "关闭全部连接")]
    CloseAll,
}

#[derive(Debug, Subcommand)]
pub enum ProviderCommand {
    #[command(about = "更新 Provider")]
    Update {
        #[arg(help = "Provider 名称")]
        provider: String,
    },
    #[command(about = "对 Provider 执行健康检查")]
    Healthcheck {
        #[arg(help = "Provider 名称")]
        provider: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum SubscriptionCommand {
    #[command(about = "更新一个、全部或到期订阅")]
    Update(SubscriptionUpdateArgs),
    #[command(about = "查看订阅启动检查、最近结果和任务")]
    Status,
}

#[derive(Debug, Subcommand)]
pub enum JobsCommand {
    #[command(about = "列出任务")]
    List,
    #[command(about = "查看任务详情")]
    Get {
        #[arg(help = "任务 ID")]
        id: String,
    },
    #[command(about = "重试支持的任务")]
    Retry {
        #[arg(help = "任务 ID")]
        id: String,
    },
    #[command(about = "取消运行中的任务")]
    Cancel {
        #[arg(help = "任务 ID")]
        id: String,
    },
}

#[derive(Debug, Args)]
pub struct SubscriptionUpdateArgs {
    #[arg(help = "订阅/Profile ID")]
    pub id: Option<String>,
    #[arg(long, help = "更新全部远程订阅")]
    pub all: bool,
    #[arg(long, help = "只更新已到期订阅")]
    pub due: bool,
}

#[derive(Debug, Args)]
pub struct DiagnoseArgs {
    #[arg(long, help = "保存脱敏诊断快照到 diagnostics 目录")]
    pub save: bool,
}

#[derive(Debug, Subcommand)]
pub enum SwitchCommand {
    #[command(about = "开启")]
    On,
    #[command(about = "关闭")]
    Off,
    #[command(about = "查看状态")]
    Status,
    #[command(about = "诊断当前平台 TUN 基本条件（无副作用）")]
    Doctor,
}

#[derive(Debug, Subcommand)]
pub enum SystemProxyCommand {
    #[command(about = "开启")]
    On,
    #[command(about = "关闭")]
    Off,
    #[command(about = "查看状态")]
    Status,
    #[command(about = "诊断当前平台自动应用能力（无副作用）")]
    Doctor,
}

impl Cli {
    #[must_use]
    pub fn parse_args() -> Self {
        let matches = Self::localized_command().get_matches();
        Self::from_arg_matches(&matches).unwrap_or_else(|err| err.exit())
    }

    #[must_use]
    fn localized_command() -> ClapCommand {
        localize_help(Self::command(), true)
    }

    pub fn options(&self) -> Result<ClashTuiOptions> {
        ClashTuiOptions::new(
            self.home_dir.clone(),
            self.resource_dir.clone(),
            self.mihomo_bin.clone(),
            self.subscription_check_interval_secs,
        )
    }

    #[must_use]
    pub const fn runs_tui(&self) -> bool {
        matches!(self.command, None | Some(Command::Tui))
    }
}

const CLI_ROOT_HELP_TEMPLATE: &str =
    "{before-help}{name} {version}\n{about-with-newline}\n用法：{usage}\n\n{all-args}{after-help}";
const CLI_SUBCOMMAND_HELP_TEMPLATE: &str = "{before-help}{about-with-newline}\n用法：{usage}\n\n{all-args}{after-help}";

fn localize_help(command: ClapCommand, is_root: bool) -> ClapCommand {
    let help_template = if is_root {
        CLI_ROOT_HELP_TEMPLATE
    } else {
        CLI_SUBCOMMAND_HELP_TEMPLATE
    };
    let command = command
        .help_template(help_template)
        .subcommand_help_heading("命令")
        .disable_help_subcommand(true);
    let command = if is_root {
        command.next_help_heading("全局选项")
    } else {
        command.disable_help_flag(true).mut_args(|arg| {
            let heading = localized_arg_heading(&arg);
            arg.help_heading(heading)
        })
    };
    command.mut_subcommands(|subcommand| localize_help(subcommand, false))
}

fn localized_arg_heading(arg: &Arg) -> &'static str {
    if arg.is_positional() { "参数" } else { "选项" }
}

pub async fn execute(cli: Cli, state: Arc<AppState>) -> i32 {
    let json_output = cli.json;
    match execute_inner(cli.command.unwrap_or(Command::Tui), state).await {
        Ok(output) => {
            write_success(&output, json_output);
            0
        }
        Err(err) => {
            write_error(&err.to_string(), json_output);
            1
        }
    }
}

async fn execute_inner(command: Command, state: Arc<AppState>) -> Result<CliOutput> {
    match command {
        Command::Tui => {
            crate::tui::run(state).await?;
            Ok(CliOutput::message("tui exited"))
        }
        Command::Core { command } => execute_core(command, &state).await,
        Command::Mode { command } => execute_mode(command, &state).await,
        Command::Profile { command } => execute_profile(command, state).await,
        Command::Proxy { command } => execute_proxy(command, &state).await,
        Command::Settings { command } => execute_settings(command, state).await,
        Command::Runtime { command } => execute_runtime(command, state).await,
        Command::Rules { command } => execute_rules(command, &state).await,
        Command::Connections { command } => execute_connections(command, &state).await,
        Command::Provider { command } => execute_provider(command, &state).await,
        Command::Subscription { command } => execute_subscription(command, state).await,
        Command::Jobs { command } => execute_jobs(command, state).await,
        Command::Diagnose(args) => {
            let report = actions::diagnose::report(&state).await;
            if args.save {
                return Ok(CliOutput::data(
                    "diagnose saved",
                    actions::diagnose::save_report(&state, &report).await?,
                ));
            }
            Ok(CliOutput::data("diagnose", report))
        }
        Command::Tun { command } => execute_tun(command, &state).await,
        Command::SystemProxy { command } => execute_system_proxy(command, &state).await,
    }
}

async fn execute_core(command: CoreCommand, state: &AppState) -> Result<CliOutput> {
    match command {
        CoreCommand::Start => Ok(CliOutput::data("core start", actions::core::start(state).await?)),
        CoreCommand::Stop => Ok(CliOutput::data("core stop", actions::core::stop(state).await?)),
        CoreCommand::Restart => Ok(CliOutput::data("core restart", actions::core::restart(state).await?)),
        CoreCommand::Run => {
            actions::core::run(state).await?;
            Ok(CliOutput::message("core run exited"))
        }
        CoreCommand::Status => Ok(CliOutput::data("core status", actions::core::status(state).await)),
        CoreCommand::Logs => Ok(CliOutput::data("core logs", actions::core::logs(state).await)),
    }
}

async fn execute_mode(command: ModeCommand, state: &AppState) -> Result<CliOutput> {
    match command {
        ModeCommand::Get => Ok(CliOutput::data("mode", actions::config::get_mode(state).await?)),
        ModeCommand::Set { mode } => Ok(CliOutput::data(
            "mode set",
            actions::config::set_mode(state, mode.into()).await?,
        )),
    }
}

async fn execute_profile(command: ProfileCommand, state: Arc<AppState>) -> Result<CliOutput> {
    match command {
        ProfileCommand::List => Ok(CliOutput::data("profiles", actions::profiles::list(&state).await?)),
        ProfileCommand::Current => Ok(CliOutput::data(
            "current profile",
            actions::profiles::current(&state).await?,
        )),
        ProfileCommand::Switch { id } => Ok(CliOutput::data(
            "profile switched",
            actions::profiles::switch(state, id).await?,
        )),
        ProfileCommand::ImportLocal(args) => {
            let file_data = tokio::fs::read_to_string(&args.file).await?;
            let input = LocalProfileImport {
                uid: args.id,
                name: args.name,
                file_data,
            };
            Ok(CliOutput::data(
                "local profile imported",
                actions::profiles::import_local(&state, &input).await?,
            ))
        }
        ProfileCommand::ImportUrl(args) => {
            let activate = args.activate || args.start_core;
            let start_core = args.start_core;
            let requested_uid = args.id.clone().or_else(|| activate.then(generate_remote_uid));
            let url = read_import_url(&args)?;
            let input = RemoteProfileImport {
                url,
                uid: requested_uid.clone(),
                name: args.name,
                desc: None,
                option: None,
            };
            if activate {
                match actions::profiles::import_remote_with_retry_and_activate(Arc::clone(&state), &input, start_core)
                    .await
                {
                    Ok(result) => {
                        return Ok(CliOutput::data(
                            "remote profile imported and activated",
                            RemoteImportOutput::from_activation(
                                &result.activation,
                                requested_uid.as_deref(),
                                Some(result.import.attempt),
                            ),
                        ));
                    }
                    Err(err) => anyhow::bail!("{}", redact_urls(&err.to_string())),
                }
            }
            match actions::profiles::import_remote_with_retry(&state, &input).await {
                Ok(imported) => Ok(CliOutput::data(
                    "remote profile imported",
                    RemoteImportOutput::from_profiles(
                        &imported.profiles,
                        requested_uid.as_deref(),
                        Some(imported.attempt),
                    ),
                )),
                Err(err) => anyhow::bail!("{}", redact_urls(&err.to_string())),
            }
        }
    }
}

fn read_import_url(args: &ImportUrlArgs) -> Result<String> {
    if args.stdin {
        let mut buffer = String::new();
        std::io::stdin().read_to_string(&mut buffer)?;
        let url = buffer.trim();
        if url.is_empty() {
            anyhow::bail!("subscription URL read from stdin is empty");
        }
        return Ok(url.to_owned());
    }

    args.url
        .as_ref()
        .map(|url| url.trim().to_owned())
        .filter(|url| !url.is_empty())
        .ok_or_else(|| anyhow::anyhow!("subscription URL is required"))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteImportOutput {
    imported: Option<RemoteImportProfileSummary>,
    current: Option<String>,
    profile_count: usize,
    attempt: Option<actions::profiles::RemoteImportAttempt>,
    activation: Option<RemoteImportActivationSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteImportActivationSummary {
    runtime_path: String,
    runtime_validated: bool,
    runtime_reloaded: bool,
    started_core: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteImportProfileSummary {
    uid: Option<String>,
    name: Option<String>,
    kind: Option<String>,
    file: Option<String>,
    updated: Option<usize>,
    url_configured: bool,
}

impl RemoteImportOutput {
    fn from_profiles(
        profiles: &IProfiles,
        requested_uid: Option<&str>,
        attempt: Option<actions::profiles::RemoteImportAttempt>,
    ) -> Self {
        let items = profiles.items.as_deref().unwrap_or_default();
        let imported = imported_profile(items, requested_uid).map(RemoteImportProfileSummary::from);
        Self {
            imported,
            current: profiles.current.clone(),
            profile_count: items.len(),
            attempt,
            activation: None,
        }
    }

    fn from_activation(
        result: &actions::profiles::ProfileSwitchResult,
        requested_uid: Option<&str>,
        attempt: Option<actions::profiles::RemoteImportAttempt>,
    ) -> Self {
        let items = result.profiles.items.as_deref().unwrap_or_default();
        let imported = imported_profile(items, requested_uid).map(RemoteImportProfileSummary::from);
        Self {
            imported,
            current: result.profiles.current.clone(),
            profile_count: items.len(),
            attempt,
            activation: Some(RemoteImportActivationSummary {
                runtime_path: result.runtime_path.clone(),
                runtime_validated: result.runtime_validated,
                runtime_reloaded: result.runtime_reloaded,
                started_core: result.started_core,
            }),
        }
    }
}

fn imported_profile<'a>(items: &'a [PrfItem], requested_uid: Option<&str>) -> Option<&'a PrfItem> {
    requested_uid
        .and_then(|uid| items.iter().find(|item| item.uid.as_deref() == Some(uid)))
        .or_else(|| items.iter().rev().find(|item| item.itype.as_deref() == Some("remote")))
}

impl From<&PrfItem> for RemoteImportProfileSummary {
    fn from(item: &PrfItem) -> Self {
        Self {
            uid: item.uid.clone(),
            name: item.name.clone(),
            kind: item.itype.clone(),
            file: item.file.clone(),
            updated: item.updated,
            url_configured: item.url.is_some(),
        }
    }
}

async fn execute_proxy(command: ProxyCommand, state: &AppState) -> Result<CliOutput> {
    match command {
        ProxyCommand::Groups => Ok(CliOutput::data(
            "proxy groups",
            actions::controller::proxy_groups(state).await?,
        )),
        ProxyCommand::Select { group, proxy } => {
            let result = actions::controller::select_or_preselect_proxy(state, &group, &proxy).await?;
            if result.preselected {
                Ok(CliOutput::message(format!(
                    "已预选 {} -> {}，Core 启动后自动应用",
                    result.group, result.proxy
                )))
            } else {
                Ok(CliOutput::message(format!(
                    "已选择 {} -> {}",
                    result.group, result.proxy
                )))
            }
        }
    }
}

async fn execute_settings(command: SettingsCommand, state: Arc<AppState>) -> Result<CliOutput> {
    match command {
        SettingsCommand::Show => Ok(CliOutput::data("settings", actions::config::settings(&state).await?)),
        SettingsCommand::Set { key, value } => match key {
            SettingsKeyArg::Dns => Ok(CliOutput::data(
                "settings updated",
                actions::config::set_dns_enabled(state, parse_bool(&value)?).await?,
            )),
            SettingsKeyArg::Ipv6 => Ok(CliOutput::data(
                "settings updated",
                actions::config::set_ipv6(&state, parse_bool(&value)?).await?,
            )),
            SettingsKeyArg::AllowLan => Ok(CliOutput::data(
                "settings updated",
                actions::config::set_allow_lan(&state, parse_bool(&value)?).await?,
            )),
            SettingsKeyArg::UnifiedDelay => Ok(CliOutput::data(
                "settings updated",
                actions::config::set_unified_delay(&state, parse_bool(&value)?).await?,
            )),
            SettingsKeyArg::LogLevel => Ok(CliOutput::data(
                "settings updated",
                actions::config::set_log_level(&state, &value).await?,
            )),
            SettingsKeyArg::CoreLog => Ok(CliOutput::data(
                "settings updated",
                actions::config::set_core_log_enabled(&state, parse_bool(&value)?).await?,
            )),
            SettingsKeyArg::MixedPort => {
                let port = value.parse::<u16>()?;
                Ok(CliOutput::data(
                    "settings updated",
                    actions::config::set_mixed_port(&state, port).await?,
                ))
            }
            SettingsKeyArg::ExternalController => Ok(CliOutput::data(
                "settings updated",
                actions::config::set_external_controller_enabled(&state, parse_bool(&value)?).await?,
            )),
            SettingsKeyArg::ExternalControllerPort => {
                let port = value.parse::<u16>()?;
                Ok(CliOutput::data(
                    "settings updated",
                    actions::config::set_external_controller_port(&state, port).await?,
                ))
            }
        },
        SettingsCommand::Dns { command } => execute_dns(command, state).await,
    }
}

async fn execute_dns(command: DnsCommand, state: Arc<AppState>) -> Result<CliOutput> {
    match command {
        DnsCommand::Show => Ok(CliOutput::data(
            "dns config",
            crate::validation::get_dns_config_content(&state).await?,
        )),
        DnsCommand::Save { file } => {
            let content = tokio::fs::read_to_string(&file).await?;
            let yaml_value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&content)?;
            let json_value = serde_json::to_value(yaml_value)?;
            Ok(CliOutput::data(
                "dns config saved",
                crate::validation::save_dns_config(&state, json_value).await?,
            ))
        }
        DnsCommand::Validate => {
            let started = crate::validation::start_dns_validation_job(Arc::clone(&state)).await;
            let job = wait_for_job(&state, &started.job.id).await?;
            Ok(CliOutput::data("dns validation", job))
        }
    }
}

async fn execute_runtime(command: RuntimeCommand, state: Arc<AppState>) -> Result<CliOutput> {
    match command {
        RuntimeCommand::Generate => Ok(CliOutput::data(
            "runtime generated",
            actions::runtime::generate(&state).await?,
        )),
        RuntimeCommand::Show => Ok(CliOutput::data(
            "runtime yaml",
            actions::runtime::read_yaml(&state).await?,
        )),
        RuntimeCommand::Validate => {
            let started = crate::validation::start_runtime_validation_job(Arc::clone(&state)).await;
            let job = wait_for_job(&state, &started.job.id).await?;
            Ok(CliOutput::data("runtime validation", job))
        }
    }
}

async fn execute_rules(command: RulesCommand, state: &AppState) -> Result<CliOutput> {
    let mut rules = actions::controller::rules(state).await?;
    match command {
        RulesCommand::List { limit } => {
            apply_rules_limit(&mut rules, limit);
            Ok(CliOutput::data("rules", rules))
        }
        RulesCommand::Search { query, limit } => {
            rules.rules.retain(|rule| rule.matches_query(&query));
            apply_rules_limit(&mut rules, limit);
            Ok(CliOutput::data("rules search", rules))
        }
    }
}

fn apply_rules_limit(rules: &mut RulesResponse, limit: Option<usize>) {
    if let Some(limit) = limit {
        rules.rules.truncate(limit);
    }
}

async fn execute_connections(command: ConnectionsCommand, state: &AppState) -> Result<CliOutput> {
    match command {
        ConnectionsCommand::List => Ok(CliOutput::data(
            "connections",
            actions::controller::connections(state).await?,
        )),
        ConnectionsCommand::Close { id } => {
            actions::controller::close_connection(state, &id).await?;
            Ok(CliOutput::message(format!("closed connection {id}")))
        }
        ConnectionsCommand::CloseAll => {
            actions::controller::close_all_connections(state).await?;
            Ok(CliOutput::message("closed all connections"))
        }
    }
}

async fn execute_provider(command: ProviderCommand, state: &AppState) -> Result<CliOutput> {
    match command {
        ProviderCommand::Update { provider } => Ok(CliOutput::data(
            "provider update",
            actions::controller::update_provider(state, &provider).await?,
        )),
        ProviderCommand::Healthcheck { provider } => Ok(CliOutput::data(
            "provider healthcheck",
            actions::controller::healthcheck_provider(state, &provider).await?,
        )),
    }
}

async fn execute_subscription(command: SubscriptionCommand, state: Arc<AppState>) -> Result<CliOutput> {
    match command {
        SubscriptionCommand::Update(args) => {
            if args.all {
                let mut sweep = actions::subscriptions::update_all(Arc::clone(&state)).await?;
                sweep.jobs = wait_for_jobs(&state, &sweep.jobs).await?;
                return Ok(CliOutput::data("subscription update all", sweep));
            }
            if args.due {
                let mut sweep = actions::subscriptions::update_due(Arc::clone(&state)).await?;
                sweep.jobs = wait_for_jobs(&state, &sweep.jobs).await?;
                return Ok(CliOutput::data("subscription update due", sweep));
            }
            let Some(id) = args.id else {
                anyhow::bail!("subscription update requires <id>, --all, or --due");
            };
            let mut started = actions::subscriptions::update_one(Arc::clone(&state), id).await;
            started.job = wait_for_job(&state, &started.job.id).await?;
            Ok(CliOutput::data("subscription update", started))
        }
        SubscriptionCommand::Status => Ok(CliOutput::data(
            "subscription status",
            actions::subscriptions::status(&state).await?,
        )),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct JobRetryReport {
    supported: bool,
    retried: bool,
    message: String,
    source: JobRecord,
    job: Option<JobRecord>,
}

async fn execute_jobs(command: JobsCommand, state: Arc<AppState>) -> Result<CliOutput> {
    match command {
        JobsCommand::List => Ok(CliOutput::data("jobs", state.jobs.list().await)),
        JobsCommand::Get { id } => {
            let Some(job) = state.jobs.get(&id).await else {
                anyhow::bail!("job not found: {id}");
            };
            Ok(CliOutput::data("job", job))
        }
        JobsCommand::Retry { id } => {
            let Some(job) = state.jobs.get(&id).await else {
                anyhow::bail!("job not found: {id}");
            };
            let Some(target) = job.target.clone().filter(|_| job.kind == "profile-update") else {
                return Ok(CliOutput::data(
                    "job retry",
                    JobRetryReport {
                        supported: false,
                        retried: false,
                        message: "only profile-update jobs can be retried in this build".into(),
                        source: job,
                        job: None,
                    },
                ));
            };
            let mut started = actions::subscriptions::update_one(Arc::clone(&state), target).await;
            started.job = wait_for_job(&state, &started.job.id).await?;
            Ok(CliOutput::data(
                "job retry",
                JobRetryReport {
                    supported: true,
                    retried: started.created,
                    message: if started.created {
                        "retry completed".into()
                    } else {
                        "an active matching job already existed; returned its latest state".into()
                    },
                    source: job,
                    job: Some(started.job),
                },
            ))
        }
        JobsCommand::Cancel { id } => {
            let Some(report) = state.jobs.cancel_report(&id).await else {
                anyhow::bail!("job not found: {id}");
            };
            Ok(CliOutput::data("job cancel", report))
        }
    }
}

async fn execute_tun(command: SwitchCommand, state: &AppState) -> Result<CliOutput> {
    match command {
        SwitchCommand::On => Ok(CliOutput::data("tun on", actions::system::set_tun(state, true).await?)),
        SwitchCommand::Off => Ok(CliOutput::data(
            "tun off",
            actions::system::set_tun(state, false).await?,
        )),
        SwitchCommand::Status => Ok(CliOutput::data("tun status", actions::system::tun_status(state).await?)),
        SwitchCommand::Doctor => Ok(CliOutput::data(
            "tun doctor",
            actions::system::tun_diagnostics(state).await?,
        )),
    }
}

async fn execute_system_proxy(command: SystemProxyCommand, state: &AppState) -> Result<CliOutput> {
    match command {
        SystemProxyCommand::On => Ok(CliOutput::data(
            "system proxy on",
            actions::system::set_system_proxy(state, true).await?,
        )),
        SystemProxyCommand::Off => Ok(CliOutput::data(
            "system proxy off",
            actions::system::set_system_proxy(state, false).await?,
        )),
        SystemProxyCommand::Status => Ok(CliOutput::data(
            "system proxy status",
            actions::system::system_proxy_status(state).await?,
        )),
        SystemProxyCommand::Doctor => Ok(CliOutput::data(
            "system proxy doctor",
            actions::system::system_proxy_diagnostics(state).await?,
        )),
    }
}

async fn wait_for_jobs(state: &AppState, jobs: &[JobRecord]) -> Result<Vec<JobRecord>> {
    let mut finished = Vec::with_capacity(jobs.len());
    for job in jobs {
        finished.push(wait_for_job(state, &job.id).await?);
    }
    Ok(finished)
}

async fn wait_for_job(state: &AppState, id: &str) -> Result<JobRecord> {
    tokio::time::timeout(Duration::from_secs(300), async {
        loop {
            if let Some(job) = state.jobs.get(id).await
                && matches!(
                    job.status,
                    JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled
                )
            {
                return Ok(job);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for job {id}"))?
}

fn parse_bool(value: &str) -> Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "enable" | "enabled" => Ok(true),
        "0" | "false" | "no" | "off" | "disable" | "disabled" => Ok(false),
        _ => anyhow::bail!("expected boolean value: on/off/true/false/1/0"),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliOutput {
    pub message: String,
    pub data: serde_json::Value,
}

impl CliOutput {
    fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            data: serde_json::Value::Null,
        }
    }

    fn data<T>(message: impl Into<String>, data: T) -> Self
    where
        T: Serialize,
    {
        let mut data = serde_json::to_value(data).unwrap_or_else(|err| {
            json!({
                "serializationError": err.to_string(),
            })
        });
        redact_json_urls(&mut data);
        Self {
            message: message.into(),
            data,
        }
    }
}

fn write_success(output: &CliOutput, json_output: bool) {
    if json_output {
        println!("{}", serde_json::to_string_pretty(output).unwrap_or_default());
    } else if output.data.is_null() {
        println!("{}", output.message);
    } else {
        println!("{}", output.message);
        println!("{}", serde_json::to_string_pretty(&output.data).unwrap_or_default());
    }
}

fn write_error(message: &str, json_output: bool) {
    let message = redact_urls(message);
    if json_output {
        eprintln!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "success": false,
                "error": {
                    "kind": "operation-failed",
                    "message": message,
                }
            }))
            .unwrap_or_default()
        );
    } else {
        eprintln!("{message}");
    }
}

fn redact_urls(message: &str) -> String {
    message
        .split_whitespace()
        .map(|part| {
            if part.starts_with("http://") || part.starts_with("https://") {
                "[redacted-url]"
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn redact_json_urls(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(value) => {
            *value = redact_urls(value);
        }
        serde_json::Value::Array(values) => {
            for value in values {
                redact_json_urls(value);
            }
        }
        serde_json::Value::Object(values) => {
            let original = std::mem::take(values);
            for (key, mut value) in original {
                redact_json_urls(&mut value);
                values.insert(redact_urls(&key), value);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use serde_json::json;

    use super::{Cli, CliOutput, Command, ProfileCommand, redact_urls};

    #[test]
    fn parses_core_status() {
        let cli = Cli::parse_from(["clash-tui", "--json", "core", "status"]);

        assert!(cli.json);
        assert!(matches!(cli.command, Some(Command::Core { .. })));
    }

    #[test]
    fn parses_default_without_subcommand() {
        let cli = Cli::parse_from(["clash-tui"]);

        assert!(cli.command.is_none());
    }

    #[test]
    fn subscription_startup_sweep_only_runs_with_tui_lifecycle() {
        let default_tui = Cli::parse_from(["clash-tui"]);
        let explicit_tui = Cli::parse_from(["clash-tui", "tui"]);
        let one_shot_cli = Cli::parse_from(["clash-tui", "core", "status"]);

        assert!(default_tui.runs_tui());
        assert!(explicit_tui.runs_tui());
        assert!(!one_shot_cli.runs_tui());
    }

    #[test]
    fn top_level_help_is_chinese_and_actionable() {
        let mut command = Cli::localized_command();
        let mut buffer = Vec::new();
        command.write_help(&mut buffer).expect("help");
        let help = String::from_utf8(buffer).expect("utf8");

        assert!(help.contains("mihomo 本地 TUI/CLI 控制器"));
        assert!(help.contains("用法："));
        assert!(help.contains("打开中文 TUI 控制台"));
        assert!(help.contains("管理 Profile 和订阅导入"));
        assert!(help.contains("clash-tui profile import-url --stdin --start-core"));
    }

    #[test]
    fn nested_help_uses_chinese_headings() {
        let import_help = render_help_from_args(["clash-tui", "profile", "import-url", "--help"]);
        assert!(import_help.contains("用法："));
        assert!(import_help.contains("参数:"));
        assert!(import_help.contains("选项:"));
        assert!(import_help.contains("全局选项:"));
        assert!(import_help.contains("从标准输入读取订阅链接"));
        assert!(!import_help.contains("clash-tui-profile-import-url"));
        assert_no_english_help_headings(&import_help);

        let tun_help = render_help_from_args(["clash-tui", "tun", "--help"]);
        assert!(tun_help.contains("命令:"));
        assert!(tun_help.contains("doctor  诊断当前平台 TUN 基本条件"));
        assert!(tun_help.contains("无副作用"));
        assert!(tun_help.contains("CAP_NET_ADMIN"));
        assert!(tun_help.contains("getcap/setcap"));
        assert!(tun_help.contains("tun off"));
        assert!(tun_help.contains("core stop"));
        assert!(!tun_help.contains("clash-tui-tun"));
        assert_no_english_help_headings(&tun_help);

        let system_proxy_help = render_help_from_args(["clash-tui", "system-proxy", "--help"]);
        assert!(system_proxy_help.contains("命令:"));
        assert!(system_proxy_help.contains("on      开启"));
        assert!(system_proxy_help.contains("doctor  诊断当前平台自动应用能力"));
        assert!(system_proxy_help.contains("manualAction"));
        assert!(system_proxy_help.contains("无副作用"));
        assert!(system_proxy_help.contains("org.gnome.system.proxy"));
        assert!(system_proxy_help.contains("system-proxy doctor"));
        assert!(system_proxy_help.contains("system-proxy off"));
        assert!(!system_proxy_help.contains("clash-tui-system-proxy"));
        assert_no_english_help_headings(&system_proxy_help);
    }

    fn render_help_from_args<const N: usize>(args: [&str; N]) -> String {
        let mut command = Cli::localized_command();
        let err = command.try_get_matches_from_mut(args).expect_err("help error");
        err.to_string()
    }

    fn assert_no_english_help_headings(help: &str) {
        for forbidden in ["Usage:", "Arguments:", "Options:", "Commands:", "Print help"] {
            assert!(
                !help.contains(forbidden),
                "help output should not contain {forbidden:?}:\n{help}"
            );
        }
    }

    #[test]
    fn parses_settings_set() {
        let cli = Cli::parse_from(["clash-tui", "settings", "set", "mixed-port", "7897"]);
        let core_log = Cli::parse_from(["clash-tui", "settings", "set", "core-log", "false"]);
        let external = Cli::parse_from(["clash-tui", "settings", "set", "external-controller", "true"]);
        let external_port = Cli::parse_from(["clash-tui", "settings", "set", "external-controller-port", "9097"]);

        assert!(matches!(cli.command, Some(Command::Settings { .. })));
        assert!(matches!(core_log.command, Some(Command::Settings { .. })));
        assert!(matches!(external.command, Some(Command::Settings { .. })));
        assert!(matches!(external_port.command, Some(Command::Settings { .. })));
    }

    #[test]
    fn parses_p1_controller_commands() {
        let rules = Cli::parse_from(["clash-tui", "rules", "search", "example", "--limit", "10"]);
        let connections = Cli::parse_from(["clash-tui", "connections", "close-all"]);
        let provider = Cli::parse_from(["clash-tui", "provider", "healthcheck", "Proxy Provider"]);

        assert!(matches!(rules.command, Some(Command::Rules { .. })));
        assert!(matches!(connections.command, Some(Command::Connections { .. })));
        assert!(matches!(provider.command, Some(Command::Provider { .. })));
    }

    #[test]
    fn parses_jobs_and_validation_commands() {
        let jobs = Cli::parse_from(["clash-tui", "jobs", "retry", "job-1"]);
        let runtime = Cli::parse_from(["clash-tui", "runtime", "validate"]);
        let dns = Cli::parse_from(["clash-tui", "settings", "dns", "validate"]);
        let diagnose = Cli::parse_from(["clash-tui", "--json", "diagnose"]);
        let diagnose_save = Cli::parse_from(["clash-tui", "diagnose", "--save"]);

        assert!(matches!(jobs.command, Some(Command::Jobs { .. })));
        assert!(matches!(runtime.command, Some(Command::Runtime { .. })));
        assert!(matches!(dns.command, Some(Command::Settings { .. })));
        assert!(matches!(diagnose.command, Some(Command::Diagnose(args)) if !args.save));
        assert!(matches!(diagnose_save.command, Some(Command::Diagnose(args)) if args.save));
    }

    #[test]
    fn parses_profile_import_url_from_stdin() {
        let cli = Cli::parse_from(["clash-tui", "profile", "import-url", "--stdin"]);
        let Some(Command::Profile {
            command: ProfileCommand::ImportUrl(args),
        }) = cli.command
        else {
            unreachable!("expected profile import-url");
        };

        assert!(args.stdin);
        assert!(args.url.is_none());
    }

    #[test]
    fn redacts_urls_from_error_output() {
        let message = redact_urls("failed to fetch https://example.test/sub?token=secret after retry");

        assert_eq!(message, "failed to fetch [redacted-url] after retry");
        assert!(!message.contains("secret"));
    }

    #[test]
    fn redacts_urls_from_cli_output_data() {
        let output = CliOutput::data(
            "profiles",
            json!({
                "items": [
                    {
                        "uid": "R1",
                        "url": "https://example.test/sub?token=secret",
                        "error": "failed https://example.test/sub?token=secret",
                        "extra": {
                            "https://example.test/generate_204": {
                                "alive": false
                            }
                        }
                    }
                ]
            }),
        );
        let rendered = serde_json::to_string(&output.data).expect("json");

        assert!(rendered.contains("[redacted-url]"));
        assert!(!rendered.contains("secret"));
        assert!(!rendered.contains("example.test/sub"));
        assert!(!rendered.contains("example.test/generate_204"));
    }
}
