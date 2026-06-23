use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    text::Line,
    widgets::{Cell, Paragraph, Row, StatefulWidget, Table, TableState, Widget as _, Wrap},
};

use crate::tui::{ProviderDialogKind, TuiApp, visible_indices_with_offset};

use super::layout::{
    clear_area, display_line, fit_display_width, render_vertical_scrollbar, selected_style, table_cell_text,
    themed_block,
};

pub fn render(area: Rect, buffer: &mut Buffer, app: &TuiApp) {
    let Some(kind) = app.provider_dialog else {
        return;
    };
    let modal = modal_rect(area);
    clear_area(modal, buffer);

    let block = themed_block(kind.title());
    let inner = block.inner(modal);
    block.render(modal, buffer);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(inner);
    render_header(chunks[0], buffer, app, kind);

    match kind {
        ProviderDialogKind::Proxy => render_proxy_providers(chunks[1], buffer, app),
        ProviderDialogKind::Rule => render_rule_providers(chunks[1], buffer, app),
    }
}

fn render_header(area: Rect, buffer: &mut Buffer, app: &TuiApp, kind: ProviderDialogKind) {
    let width = usize::from(area.width).max(1);
    let count = app.provider_len(kind);
    let selected = app.selected_provider_name(kind).unwrap_or_else(|| "未选择".to_owned());
    let lines = vec![
        Line::from(fit_display_width(
            &format!("{}：{} 个 | 当前：{}", kind.label(), count, selected),
            width,
        )),
        Line::from(fit_display_width(
            "操作：↑↓ 选择 | Enter/u 更新选中 | a 更新全部 | r 刷新 | Esc 关闭",
            width,
        )),
        Line::from(""),
    ];
    Paragraph::new(lines).wrap(Wrap { trim: true }).render(area, buffer);
}

fn render_proxy_providers(area: Rect, buffer: &mut Buffer, app: &TuiApp) {
    if app.proxy_providers.is_empty() {
        Paragraph::new(display_line("当前配置没有 Proxy Provider")).render(area, buffer);
        return;
    }

    let indices = (0..app.proxy_providers.len()).collect::<Vec<_>>();
    let row_count = table_row_count(area);
    let (visible, offset) = visible_indices_with_offset(&indices, app.proxy_provider_index, row_count);
    let selected = visible.iter().position(|index| *index == app.proxy_provider_index);
    let rows = visible.iter().filter_map(|index| {
        let provider = app.proxy_providers.get(*index)?;
        let current = if *index == app.proxy_provider_index { ">" } else { " " };
        let provider_type = provider_display_type(&provider.vehicle_type, &provider.provider_type);
        let feedback = app
            .provider_feedback(ProviderDialogKind::Proxy, &provider.name)
            .unwrap_or("-");
        Some(Row::new(vec![
            Cell::from(current),
            Cell::from(table_cell_text(&provider.name)),
            Cell::from(provider.proxy_count.to_string()),
            Cell::from(table_cell_text(&provider_type)),
            Cell::from(table_cell_text(&format_provider_traffic(
                provider.subscription.as_ref(),
            ))),
            Cell::from(table_cell_text(&format_provider_expire(provider.subscription.as_ref()))),
            Cell::from(table_cell_text(display_updated_at(provider.updated_at.as_deref()))),
            Cell::from(table_cell_text(feedback)),
        ]))
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Min(20),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(22),
            Constraint::Length(10),
            Constraint::Length(18),
            Constraint::Length(12),
        ],
    )
    .header(Row::new(vec![
        "", "名称", "节点", "类型", "流量", "到期", "更新", "状态",
    ]))
    .row_highlight_style(selected_style());
    let mut state = TableState::default().with_selected(selected);
    StatefulWidget::render(table, area, buffer, &mut state);
    render_vertical_scrollbar(area, buffer, indices.len(), row_count, offset);
}

fn render_rule_providers(area: Rect, buffer: &mut Buffer, app: &TuiApp) {
    if app.rule_providers.is_empty() {
        Paragraph::new(display_line("当前配置没有 Rule Provider")).render(area, buffer);
        return;
    }

    let indices = (0..app.rule_providers.len()).collect::<Vec<_>>();
    let row_count = table_row_count(area);
    let (visible, offset) = visible_indices_with_offset(&indices, app.rule_provider_index, row_count);
    let selected = visible.iter().position(|index| *index == app.rule_provider_index);
    let rows = visible.iter().filter_map(|index| {
        let provider = app.rule_providers.get(*index)?;
        let current = if *index == app.rule_provider_index { ">" } else { " " };
        let provider_type = provider_display_type(&provider.vehicle_type, &provider.provider_type);
        let feedback = app
            .provider_feedback(ProviderDialogKind::Rule, &provider.name)
            .unwrap_or("-");
        Some(Row::new(vec![
            Cell::from(current),
            Cell::from(table_cell_text(&provider.name)),
            Cell::from(provider.rule_count.to_string()),
            Cell::from(table_cell_text(&provider_type)),
            Cell::from(table_cell_text(&provider.behavior)),
            Cell::from(table_cell_text(&provider.format)),
            Cell::from(table_cell_text(display_updated_at(provider.updated_at.as_deref()))),
            Cell::from(table_cell_text(feedback)),
        ]))
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Min(22),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Length(18),
            Constraint::Length(12),
        ],
    )
    .header(Row::new(vec![
        "", "名称", "规则", "类型", "行为", "格式", "更新", "状态",
    ]))
    .row_highlight_style(selected_style());
    let mut state = TableState::default().with_selected(selected);
    StatefulWidget::render(table, area, buffer, &mut state);
    render_vertical_scrollbar(area, buffer, indices.len(), row_count, offset);
}

fn modal_rect(area: Rect) -> Rect {
    let max_width = area.width.saturating_sub(4).max(1);
    let min_width = 78.min(max_width);
    let width = area.width.saturating_mul(88).saturating_div(100);
    let width = width.clamp(min_width, max_width);
    let height = area.height.saturating_sub(4).clamp(16, 28);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn table_row_count(area: Rect) -> usize {
    usize::from(area.height.saturating_sub(1)).max(1)
}

fn provider_display_type(vehicle_type: &str, provider_type: &str) -> String {
    if vehicle_type.trim().is_empty() || vehicle_type == "-" {
        provider_type.to_owned()
    } else {
        vehicle_type.to_owned()
    }
}

fn display_updated_at(updated_at: Option<&str>) -> &str {
    match updated_at.map(str::trim) {
        Some(value) if !value.is_empty() && !value.starts_with("0001-01-01") => value,
        _ => "-",
    }
}

fn format_provider_traffic(subscription: Option<&crate::tui::ProviderSubscriptionInfoRow>) -> String {
    let Some(subscription) = subscription else {
        return "-".to_owned();
    };
    let used = subscription
        .upload
        .unwrap_or(0)
        .saturating_add(subscription.download.unwrap_or(0));
    let total = subscription.total.unwrap_or(0);
    if total == 0 && used == 0 {
        return "-".to_owned();
    }
    if total == 0 {
        return format_bytes(used);
    }
    let percent = used.saturating_mul(1000).checked_div(total).unwrap_or(0) as f64 / 10.0;
    format!("{}/{} {:.1}%", format_bytes(used), format_bytes(total), percent)
}

fn format_provider_expire(subscription: Option<&crate::tui::ProviderSubscriptionInfoRow>) -> String {
    let expire = subscription.and_then(|subscription| subscription.expire).unwrap_or(0);
    if expire == 0 {
        return "-".to_owned();
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    if expire <= now {
        return "已到期".to_owned();
    }
    let days = (expire - now) / 86_400;
    if days == 0 {
        "今天".to_owned()
    } else if days > 3650 {
        "长期有效".to_owned()
    } else {
        format!("{days}天")
    }
}

fn format_bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = value as f64;
    let mut unit = 0usize;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} {}", UNITS[unit])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}
