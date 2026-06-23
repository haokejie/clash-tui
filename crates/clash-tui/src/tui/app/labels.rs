use clash_core::KernelState;

use crate::{actions, actions::config::SettingsSummary, jobs::JobStatus, mihomo_controller::Mode};

use super::models::{SETTINGS_ROWS, SettingRow};

pub(crate) const fn setting_label(row: SettingRow) -> &'static str {
    match row {
        SettingRow::Dns => "DNS 覆写",
        SettingRow::Ipv6 => "IPv6",
        SettingRow::AllowLan => "允许局域网",
        SettingRow::UnifiedDelay => "统一延迟",
        SettingRow::TuiTheme => "主题",
        SettingRow::TuiDisplayMode => "终端显示",
        SettingRow::TuiPunctuationMode => "中文标点",
        SettingRow::LogLevel => "日志等级",
        SettingRow::CoreLog => "核心日志",
        SettingRow::MixedPort => "混合端口",
        SettingRow::ExternalController => "外部控制器",
        SettingRow::ExternalControllerPort => "外部控制端口",
        SettingRow::Tun => "TUN",
        SettingRow::SystemProxy => "系统代理",
    }
}

pub(crate) fn setting_value(row: SettingRow, settings: &SettingsSummary) -> String {
    match row {
        SettingRow::Dns => bool_label(settings.dns_enabled).into(),
        SettingRow::Ipv6 => bool_label(settings.ipv6).into(),
        SettingRow::AllowLan => bool_label(settings.allow_lan).into(),
        SettingRow::UnifiedDelay => bool_label(settings.unified_delay).into(),
        SettingRow::TuiTheme => {
            if settings.tui_theme.overridden {
                format!("{}(临时)", settings.tui_theme.effective_label)
            } else {
                settings.tui_theme.configured_label.clone()
            }
        }
        SettingRow::TuiDisplayMode => {
            if settings.tui_display_mode.overridden {
                format!("{}(临时)", settings.tui_display_mode.effective_label)
            } else {
                settings.tui_display_mode.configured_label.clone()
            }
        }
        SettingRow::TuiPunctuationMode => {
            if settings.tui_punctuation_mode.overridden {
                format!("{}(临时)", settings.tui_punctuation_mode.effective_label)
            } else {
                settings.tui_punctuation_mode.configured_label.clone()
            }
        }
        SettingRow::LogLevel => settings.log_level.clone(),
        SettingRow::CoreLog => bool_label(settings.core_log_enabled).into(),
        SettingRow::MixedPort => settings.mixed_port.to_string(),
        SettingRow::ExternalController => {
            if settings.external_controller.enabled {
                if settings.external_controller.unsafe_bind {
                    "开启(远程)".into()
                } else {
                    "开启".into()
                }
            } else {
                "关闭".into()
            }
        }
        SettingRow::ExternalControllerPort => settings.external_controller.port.to_string(),
        SettingRow::Tun => bool_label(settings.tun_enabled).into(),
        SettingRow::SystemProxy => bool_label(settings.system_proxy_enabled).into(),
    }
}

pub(crate) const fn settings_rows() -> &'static [SettingRow] {
    &SETTINGS_ROWS
}

pub(crate) fn next_log_level(current: &str) -> &'static str {
    match current {
        "debug" => "info",
        "info" => "warning",
        "warning" => "error",
        "error" => "silent",
        _ => "debug",
    }
}

pub(crate) const fn bool_label(enabled: bool) -> &'static str {
    if enabled { "开启" } else { "关闭" }
}

pub(crate) const fn alive_label(alive: bool) -> &'static str {
    if alive { "可用" } else { "不可用" }
}

pub(crate) fn seconds_until_label(seconds: u64) -> String {
    match seconds {
        0 => "现在".into(),
        1..=59 => format!("{seconds}秒后"),
        60..=3599 => format!("{}分钟后", seconds.div_ceil(60)),
        3600..=86399 => {
            let total_minutes = seconds.div_ceil(60);
            let hours = total_minutes / 60;
            let minutes = total_minutes % 60;
            if hours >= 24 {
                "1天后".into()
            } else if minutes == 0 {
                format!("{hours}小时后")
            } else {
                format!("{hours}小时{minutes}分钟后")
            }
        }
        _ => format!("{}天后", seconds.div_ceil(86_400)),
    }
}

pub(crate) fn switch_status_message(label: &str, status: &actions::system::SwitchStatus) -> String {
    let mut parts = vec![format!(
        "{label}已{}（平台：{}）",
        bool_label(status.enabled),
        status.platform
    )];
    if status.config_saved {
        parts.push("配置已保存".into());
    }
    if status.runtime_generated {
        parts.push("runtime 已生成".into());
    }
    if status.core_restarted {
        parts.push("Core 已重启".into());
    } else if status.requires_core_restart {
        parts.push("启动或重启 Core 后生效".into());
    }
    match status.runtime_applied {
        Some(true) => parts.push("runtime 已应用".into()),
        Some(false) if status.runtime_generated && !status.requires_core_restart => {
            parts.push("Core 未运行".into());
        }
        Some(false) => {}
        None => {}
    }
    match status.platform_applied {
        Some(true) => parts.push("平台已应用".into()),
        Some(false) => parts.push("平台未应用".into()),
        None => {}
    }
    if let Some(action) = status
        .manual_action
        .as_deref()
        .filter(|action| !action.trim().is_empty())
    {
        parts.push(format!("处理建议：{action}"));
    }
    parts.push(status.message.clone());
    parts.join("；")
}

pub(crate) fn external_controller_status_message(
    label: &str,
    status: &actions::config::ExternalControllerApplyStatus,
) -> String {
    if status.message.trim().is_empty() {
        format!("{label}已保存")
    } else {
        status.message.clone()
    }
}

pub(crate) const fn bool_action_label(enabled: bool) -> &'static str {
    if enabled { "开启" } else { "关闭" }
}

pub(crate) const fn accepted_label(accepted: bool) -> &'static str {
    if accepted { "已受理" } else { "未受理" }
}

pub(crate) const fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Rule => "规则",
        Mode::Global => "全局",
        Mode::Direct => "直连",
    }
}

pub(crate) const fn kernel_state_label(state: KernelState) -> &'static str {
    match state {
        KernelState::Stopped => "已停止",
        KernelState::Starting => "启动中",
        KernelState::Running => "运行中",
        KernelState::Stopping => "停止中",
        KernelState::Restarting => "重启中",
        KernelState::Updating => "更新中",
        KernelState::Unhealthy => "异常",
        KernelState::Crashed => "已崩溃",
    }
}

pub(crate) const fn job_status_label(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Pending => "等待中",
        JobStatus::Running => "运行中",
        JobStatus::Succeeded => "成功",
        JobStatus::Failed => "失败",
        JobStatus::Cancelled => "已取消",
    }
}
