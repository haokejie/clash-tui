use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    text::Line,
    widgets::{Cell, Paragraph, Row, StatefulWidget, Table, TableState, Widget as _},
};

use crate::tui::{
    SettingRow, TuiApp, bool_label, content_rows, setting_label, setting_value, settings_rows, visible_indices,
};

use super::layout::{display_line, fit_display_width, selected_style, table_cell_text, themed_block};

pub fn render(area: Rect, buffer: &mut ratatui::buffer::Buffer, app: &TuiApp) {
    let mut lines = Vec::new();
    let Some(settings) = app.settings.as_ref() else {
        Paragraph::new(display_line("设置不可用：刷新失败或状态正忙"))
            .block(themed_block("设置"))
            .render(area, buffer);
        return;
    };
    let content_width = usize::from(area.width.saturating_sub(4)).max(32);
    let show_description = content_width >= 76;

    lines.push(Line::from(fit_display_width(
        &format!("主目录：{}", settings.paths.home_dir),
        content_width,
    )));
    lines.push(Line::from(fit_display_width(
        &controller_channel_line(settings, &app.controller_status),
        content_width,
    )));
    lines.push(Line::from(fit_display_width(
        &external_controller_line(settings),
        content_width,
    )));
    if let Some(subscription) = app.subscription_status.as_ref() {
        lines.push(Line::from(fit_display_width(
            &format!(
                "订阅启动检查：{} | 后台定时器：{} | 到期才更新",
                bool_label(subscription.scheduler.startup_check_enabled),
                bool_label(subscription.scheduler.enabled)
            ),
            content_width,
        )));
    } else {
        lines.push(display_line("订阅启动检查：尚未刷新"));
    }
    lines.push(Line::from(fit_display_width(
        "网络接管：系统代理修改桌面代理设置；TUN 由 mihomo runtime 接管全局流量",
        content_width,
    )));
    lines.push(Line::from(fit_display_width(
        &system_proxy_diagnostics_line(settings),
        content_width,
    )));
    lines.push(Line::from(fit_display_width(
        &tun_diagnostics_line(settings),
        content_width,
    )));
    lines.push(Line::from(fit_display_width(
        "生效边界：Core 运行中切换 TUN/外部控制器会重启 Core；Core 未运行则下次启动生效",
        content_width,
    )));
    lines.push(display_line("操作：Enter 切换/循环 | e 编辑选中值"));
    let block = themed_block("设置");
    let inner = block.inner(area);
    block.render(area, buffer);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(lines.len() as u16), Constraint::Min(1)])
        .split(inner);
    Paragraph::new(lines).render(chunks[0], buffer);

    let indices = (0..settings_rows().len()).collect::<Vec<_>>();
    let max_rows = content_rows(area, 8);
    let visible = visible_indices(&indices, app.setting_index, max_rows);
    let rows = visible
        .iter()
        .filter_map(|index| {
            let row = *settings_rows().get(*index)?;
            let marker = if *index == app.setting_index { ">" } else { " " };
            if show_description {
                Some(Row::new(vec![
                    Cell::from(marker),
                    Cell::from(table_cell_text(setting_label(row))),
                    Cell::from(table_cell_text(&setting_value(row, settings))),
                    Cell::from(table_cell_text(setting_action(row))),
                    Cell::from(table_cell_text(setting_description(row))),
                ]))
            } else {
                Some(Row::new(vec![
                    Cell::from(marker),
                    Cell::from(table_cell_text(setting_label(row))),
                    Cell::from(table_cell_text(&setting_value(row, settings))),
                    Cell::from(table_cell_text(setting_action(row))),
                ]))
            }
        })
        .collect::<Vec<_>>();
    let selected = visible.iter().position(|index| *index == app.setting_index);

    let table = if show_description {
        Table::new(
            rows,
            [
                Constraint::Length(2),
                Constraint::Length(16),
                Constraint::Length(14),
                Constraint::Length(8),
                Constraint::Min(18),
            ],
        )
        .header(Row::new(vec!["", "设置项", "当前值", "操作", "说明"]))
    } else {
        Table::new(
            rows,
            [
                Constraint::Length(2),
                Constraint::Length(16),
                Constraint::Length(14),
                Constraint::Min(8),
            ],
        )
        .header(Row::new(vec!["", "设置项", "当前值", "操作"]))
    }
    .row_highlight_style(selected_style());
    let mut state = TableState::default().with_selected(selected);
    StatefulWidget::render(table, chunks[1], buffer, &mut state);
}

const fn setting_action(row: SettingRow) -> &'static str {
    match row {
        SettingRow::MixedPort | SettingRow::ExternalControllerPort => "编辑",
        SettingRow::CoreLog | SettingRow::ExternalController | SettingRow::Tun | SettingRow::SystemProxy => "确认",
        SettingRow::TuiTheme
        | SettingRow::TuiDisplayMode
        | SettingRow::TuiPunctuationMode
        | SettingRow::RuleProviderDownloadProxy
        | SettingRow::LogLevel => "循环",
        _ => "切换",
    }
}

const fn setting_description(row: SettingRow) -> &'static str {
    match row {
        SettingRow::Dns => "覆写 runtime DNS 配置",
        SettingRow::Ipv6 => "写入 runtime IPv6 开关",
        SettingRow::AllowLan => "允许局域网访问 mixed-port",
        SettingRow::UnifiedDelay => "统一测速延迟计算",
        SettingRow::TuiTheme => "深橙/蓝色；统一全局背景",
        SettingRow::TuiDisplayMode => "标准/基础线框",
        SettingRow::TuiPunctuationMode => "保留/优化标点/常见标点",
        SettingRow::RuleProviderDownloadProxy => "规则 Provider 下载走配置或直连",
        SettingRow::LogLevel => "循环 debug/info/warning/error/silent",
        SettingRow::CoreLog => "控制 mihomo 日志落盘；运行中需重启",
        SettingRow::MixedPort => "编辑本机混合代理端口",
        SettingRow::ExternalController => "mihomo REST；默认关闭",
        SettingRow::ExternalControllerPort => "仅外部控制器开启后监听",
        SettingRow::Tun => "需确认；Core 运行中会重启",
        SettingRow::SystemProxy => "需确认；修改桌面代理",
    }
}

fn controller_channel_line(settings: &crate::actions::config::SettingsSummary, controller_status: &str) -> String {
    format!(
        "控制通道：{} | 状态：{} | 请求超时：{}",
        controller_channel_label(&settings.controller_endpoint),
        controller_state_label(controller_status),
        format_controller_timeout(settings.controller_timeout_millis)
    )
}

fn controller_channel_label(endpoint: &str) -> &'static str {
    if endpoint.starts_with("Unix") {
        "本机 Socket"
    } else {
        "未知"
    }
}

fn controller_state_label(controller_status: &str) -> String {
    if controller_status.starts_with("控制器：健康，") {
        "正常".into()
    } else if controller_status.contains("failed to connect mihomo unix socket") {
        "未连接".into()
    } else if controller_status.contains("Permission denied") || controller_status.contains("权限") {
        "权限不足".into()
    } else if let Some(message) = controller_status.strip_prefix("控制器：") {
        if message.contains("timed out") || message.contains("timeout") || message.contains("超时") {
            "健康检查超时".into()
        } else {
            message.to_owned()
        }
    } else {
        controller_status.to_owned()
    }
}

fn external_controller_line(settings: &crate::actions::config::SettingsSummary) -> String {
    let controller = &settings.external_controller;
    let state = if controller.enabled { "开启" } else { "关闭" };
    let mut line = if controller.enabled {
        format!(
            "外部控制：{} | {}:{} | 来源：{}",
            state,
            controller.host,
            controller.port,
            external_controller_source_label(&controller.source)
        )
    } else {
        format!(
            "外部控制：{} | 端口 {} | 来源：{}",
            state,
            controller.port,
            external_controller_source_label(&controller.source)
        )
    };
    if let Some(warning) = controller
        .warning
        .as_deref()
        .filter(|warning| !warning.trim().is_empty())
    {
        line.push_str(" | ");
        line.push_str(warning);
    }
    line
}

fn external_controller_source_label(source: &str) -> &'static str {
    match source {
        "mihomo" => "运行中",
        _ => "配置",
    }
}

fn format_controller_timeout(timeout_millis: u64) -> String {
    if timeout_millis >= 1000 && timeout_millis.is_multiple_of(1000) {
        format!("{}秒", timeout_millis / 1000)
    } else {
        format!("{timeout_millis}ms")
    }
}

fn system_proxy_diagnostics_line(settings: &crate::actions::config::SettingsSummary) -> String {
    let diagnostics = &settings.system_proxy_diagnostics;
    if diagnostics.can_auto_apply {
        return format!(
            "系统代理诊断：自动应用可用 | HTTP/HTTPS/SOCKS {}:{}",
            diagnostics.endpoint.host, diagnostics.endpoint.port
        );
    }
    let endpoint = format!(
        "HTTP/HTTPS/SOCKS {}:{} | 忽略主机 {}",
        diagnostics.endpoint.host, diagnostics.endpoint.port, diagnostics.endpoint.bypass
    );
    let reason = diagnostics
        .checks
        .iter()
        .find(|check| !check.ok)
        .map(|check| check.message.as_str())
        .unwrap_or(diagnostics.message.as_str());
    format!("系统代理诊断：需手动配置 | {endpoint} | {reason}")
}

fn tun_diagnostics_line(settings: &crate::actions::config::SettingsSummary) -> String {
    let diagnostics = &settings.tun_diagnostics;
    if diagnostics.can_enable {
        return "TUN 诊断：基本条件可用 | /dev/net/tun 与权限已满足".into();
    }
    let reason = diagnostics
        .checks
        .iter()
        .find(|check| !check.ok)
        .map(|check| check.message.as_str())
        .unwrap_or(diagnostics.message.as_str());
    format!("TUN 诊断：需处理环境 | {reason}")
}

#[cfg(test)]
mod tests {
    use super::{
        controller_channel_label, controller_state_label, external_controller_source_label, format_controller_timeout,
    };

    #[test]
    fn controller_channel_label_hides_unix_debug_path() {
        assert_eq!(
            controller_channel_label(r#"Unix { path: "/var/lib/clash-tui/app_settings/mihomo.sock" }"#),
            "本机 Socket"
        );
    }

    #[test]
    fn controller_state_label_separates_healthy_state_from_timeout_config() {
        assert_eq!(controller_state_label("控制器：健康，版本 v1.19.27"), "正常");
        assert_eq!(controller_state_label("控制器：健康检查超时"), "健康检查超时");
        assert_eq!(
            controller_state_label(
                "控制器：failed to connect mihomo unix socket /var/lib/clash-tui/app_settings/mihomo.sock"
            ),
            "未连接"
        );
    }

    #[test]
    fn format_controller_timeout_uses_request_timeout_copy() {
        assert_eq!(format_controller_timeout(5000), "5秒");
        assert_eq!(format_controller_timeout(1500), "1500ms");
    }

    #[test]
    fn external_controller_source_uses_user_facing_copy() {
        assert_eq!(external_controller_source_label("mihomo"), "运行中");
        assert_eq!(external_controller_source_label("config"), "配置");
    }
}
