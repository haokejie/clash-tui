use std::{
    env,
    time::{SystemTime, UNIX_EPOCH},
};

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Rect},
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, StatefulWidget, Table, TableState, Widget},
};

use clash_core::KernelState;

use crate::{
    mihomo_controller::Mode,
    tui::{
        DashboardProxyPopup, TuiApp, alive_label, kernel_owner_label, kernel_state_label, visible_indices_with_offset,
    },
};

use super::layout::{
    accent_style, border_style, cross_symbol, display_line, display_text, display_width, fit_display_width,
    format_proxy_delay, horizontal_symbol, muted_style, ok_style, pad_right_display, panel_style, progress_empty_style,
    progress_symbols, proxy_delay_style, render_vertical_scrollbar, selected_style, table_cell_text, table_text,
    tee_bottom_symbol, tee_left_symbol, tee_right_symbol, tee_top_symbol, themed_block, title_style, vertical_symbol,
    warn_style,
};

const LABEL_WIDTH: usize = 10;
const APP_VERSION_LABEL: &str = concat!("v", env!("CLASH_TUI_APP_VERSION"));

#[derive(Debug, Clone, Copy)]
struct DashboardSection {
    raw: Rect,
    body: Rect,
}

pub fn render(area: Rect, buffer: &mut Buffer, app: &TuiApp) {
    let block = themed_block("总览");
    let inner = block.inner(area);
    block.render(area, buffer);
    if inner.width < 20 || inner.height < 8 {
        Paragraph::new(super::layout::display_text("总览空间不足，请放大终端窗口。")).render(inner, buffer);
        return;
    }

    let (overview, proxy, switches, mode) = dashboard_cells(inner);
    draw_cross(inner, buffer);
    draw_section_title(overview.raw, buffer, "运行概览", tee_left_symbol(), cross_symbol());
    draw_section_title(proxy.raw, buffer, "快捷节点", cross_symbol(), tee_right_symbol());
    draw_section_title(switches.raw, buffer, "快速开关", tee_left_symbol(), cross_symbol());
    draw_section_title(mode.raw, buffer, "模式切换", cross_symbol(), tee_right_symbol());
    render_overview(overview.body, buffer, app);
    render_proxy_summary(proxy.body, buffer, app);
    render_switches(switches.body, buffer, app);
    render_mode(mode.body, buffer, app);
    render_proxy_popup(area, proxy.body, buffer, app);
}

fn dashboard_cells(inner: Rect) -> (DashboardSection, DashboardSection, DashboardSection, DashboardSection) {
    let left_width = (inner.width / 2).max(1);
    let top_height = ((u32::from(inner.height) * 3) / 5) as u16;
    let top_height = top_height.clamp(5, inner.height.saturating_sub(3).max(1));
    let cross_x = inner.x + left_width;
    let cross_y = inner.y + top_height;

    let left_w = cross_x.saturating_sub(inner.x);
    let right_w = inner
        .x
        .saturating_add(inner.width)
        .saturating_sub(cross_x.saturating_add(1));
    let top_h = cross_y.saturating_sub(inner.y);
    let bottom_h = inner
        .y
        .saturating_add(inner.height)
        .saturating_sub(cross_y.saturating_add(1));

    (
        dashboard_section(Rect::new(inner.x, inner.y, left_w, top_h)),
        dashboard_section(Rect::new(cross_x.saturating_add(1), inner.y, right_w, top_h)),
        dashboard_section(Rect::new(inner.x, cross_y.saturating_add(1), left_w, bottom_h)),
        dashboard_section(Rect::new(
            cross_x.saturating_add(1),
            cross_y.saturating_add(1),
            right_w,
            bottom_h,
        )),
    )
}

const fn dashboard_section(raw: Rect) -> DashboardSection {
    DashboardSection {
        raw,
        body: Rect::new(
            raw.x.saturating_add(2),
            raw.y.saturating_add(2),
            raw.width.saturating_sub(4),
            raw.height.saturating_sub(3),
        ),
    }
}

fn draw_cross(inner: Rect, buffer: &mut Buffer) {
    let style = border_style();
    let left_width = (inner.width / 2).max(1);
    let top_height = ((u32::from(inner.height) * 3) / 5) as u16;
    let top_height = top_height.clamp(5, inner.height.saturating_sub(3).max(1));
    let cross_x = inner.x + left_width;
    let cross_y = inner.y + top_height;
    let left_edge = inner.x.saturating_sub(1);
    let right_edge = inner.right();
    let top_edge = inner.y.saturating_sub(1);
    let bottom_edge = inner.bottom();

    for y in inner.y..inner.bottom() {
        buffer[(cross_x, y)].set_symbol(vertical_symbol()).set_style(style);
    }
    for x in inner.x..inner.right() {
        buffer[(x, cross_y)].set_symbol(horizontal_symbol()).set_style(style);
    }
    buffer[(cross_x, top_edge)]
        .set_symbol(tee_top_symbol())
        .set_style(style);
    buffer[(cross_x, bottom_edge)]
        .set_symbol(tee_bottom_symbol())
        .set_style(style);
    buffer[(left_edge, cross_y)]
        .set_symbol(tee_left_symbol())
        .set_style(style);
    buffer[(right_edge, cross_y)]
        .set_symbol(tee_right_symbol())
        .set_style(style);
    buffer[(cross_x, cross_y)].set_symbol(cross_symbol()).set_style(style);
}

fn draw_section_title(raw: Rect, buffer: &mut Buffer, title: &'static str, left_joint: &str, right_joint: &str) {
    if raw.width < 4 || raw.height < 2 {
        return;
    }

    let y = raw.y.saturating_add(1);
    if y >= raw.bottom() {
        return;
    }

    let left = raw.x.saturating_sub(1);
    let right = raw.right();
    if right <= left {
        return;
    }

    let style = border_style();
    for x in left..=right {
        buffer[(x, y)].set_symbol(horizontal_symbol()).set_style(style);
    }
    buffer[(left, y)].set_symbol(left_joint).set_style(style);
    buffer[(right, y)].set_symbol(right_joint).set_style(style);

    let label = title.to_string();
    let label_width = display_width(&label) as u16;
    let available = right.saturating_sub(left.saturating_add(1));
    if available == 0 {
        return;
    }
    let label_x = raw.x.saturating_add(2).min(right.saturating_sub(1));
    let label_area = Rect::new(label_x, y, label_width.min(available), 1);
    Paragraph::new(Line::from(Span::styled(display_text(&label), title_style()))).render(label_area, buffer);
}

fn render_overview(area: Rect, buffer: &mut Buffer, app: &TuiApp) {
    let width = usize::from(area.width).max(1);
    let mut lines = Vec::new();
    let (core_value, core_state) = core_summary(app);
    let (owner_value, owner_detail) = core_owner_summary(app);
    lines.push(key_value_line("核心", &core_value, Some(core_state), width));
    lines.push(key_value_line("客户端", APP_VERSION_LABEL, None, width));
    lines.push(key_value_line("管理方", &owner_value, owner_detail, width));
    lines.push(key_value_line(
        "实时",
        &format!(
            "↓ {} / ↑ {}",
            format_speed(app.dashboard_metrics.download_speed),
            format_speed(app.dashboard_metrics.upload_speed)
        ),
        Some(format!(
            "内存 {}",
            app.dashboard_metrics.memory.map_or_else(|| "未知".into(), format_bytes)
        )),
        width,
    ));
    let usage = subscription_usage(app);
    lines.push(key_value_line("订阅流量", &usage.summary, usage.profile.clone(), width));
    lines.push(progress_line("用量进度", usage.percent, width));
    lines.push(key_value_line("订阅更新", &usage.updated, None, width));
    lines.push(key_value_line(
        "终端类型",
        &terminal_type_label(),
        Some(format!(
            "显示 {}",
            crate::terminal_display::current_display_mode().label()
        )),
        width,
    ));

    render_clipped_lines(area, buffer, lines);
}

fn render_proxy_summary(area: Rect, buffer: &mut Buffer, app: &TuiApp) {
    let width = usize::from(area.width).max(1);
    let mut lines = Vec::new();
    if let Some(group) = app.dashboard_proxy_group() {
        let node = if group.now.trim().is_empty() { "-" } else { &group.now };
        let meta = app.proxy_node_meta.get(node);
        let delay = if group.offline {
            "-".into()
        } else {
            format_proxy_delay(meta.and_then(|meta| meta.delay_ms))
        };
        let alive = if group.offline {
            "启动后应用"
        } else {
            meta.and_then(|meta| meta.alive).map(alive_label).unwrap_or("未知")
        };
        lines.push(key_value_line("代理组", &table_text(&group.name, 28), None, width));
        lines.push(key_value_line_with_tail_spans(
            "当前节点",
            &table_text(node, 30),
            proxy_status_tail_spans(
                group.offline,
                meta.and_then(|meta| meta.delay_ms),
                meta.and_then(|meta| meta.alive),
                &delay,
                alive,
            ),
            width,
        ));
        lines.push(Line::from(vec![Span::styled(
            fit_display_width("g 展开组  Enter 展开节点  3 完整代理页", width),
            muted_style(),
        )]));
    } else {
        lines.push(display_line("尚未加载策略组；按 s 启动 Core 或按 r 刷新。"));
        lines.push(display_line("按 3 进入完整代理页查看诊断详情。"));
    }
    render_clipped_lines(area, buffer, lines);
}

fn render_switches(area: Rect, buffer: &mut Buffer, app: &TuiApp) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let core_state = app
        .kernel_snapshot
        .as_ref()
        .map_or("未刷新", |snapshot| kernel_state_label(snapshot.state));
    let core_action = if app.kernel_snapshot.as_ref().is_some_and(|snapshot| {
        matches!(
            snapshot.state,
            KernelState::Running | KernelState::Starting | KernelState::Restarting | KernelState::Unhealthy
        )
    }) {
        "停止核心"
    } else {
        "启动核心"
    };
    let mut rows = vec![switch_table_row("Core", core_state, "s", core_action)];
    if let Some(settings) = app.settings.as_ref() {
        rows.push(switch_table_row(
            "系统代理",
            bracket_bool(settings.system_proxy_enabled),
            "P",
            "修改桌面代理",
        ));
        rows.push(switch_table_row(
            "TUN",
            bracket_bool(settings.tun_enabled),
            "T",
            "确认后切换",
        ));
        rows.push(switch_table_row(
            "DNS",
            bracket_bool(settings.dns_enabled),
            "d",
            "切换设置",
        ));
    } else {
        rows.push(switch_table_row("设置", "未刷新", "r", "刷新后可切换"));
    }

    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Min(8),
        ],
    )
    .header(Row::new(["项目", "状态", "按键", "说明"]));
    Widget::render(table, area, buffer);
}

fn render_mode(area: Rect, buffer: &mut Buffer, app: &TuiApp) {
    let width = usize::from(area.width).max(1);
    let mut lines = Vec::new();
    let current = app.mode.unwrap_or(Mode::Rule);
    let labels = [(Mode::Rule, "规则"), (Mode::Global, "全局"), (Mode::Direct, "直连")];
    let mut spans = Vec::new();
    for (mode, label) in labels {
        let style = if mode == current {
            selected_style()
        } else {
            accent_style()
        };
        spans.push(Span::styled(format!(" {label} "), style));
        spans.push(Span::raw("  "));
    }
    lines.push(Line::from(spans));
    lines.push(Line::from(vec![Span::styled(
        fit_display_width("m 循环切换；当前配置会同步到 mihomo controller。", width),
        muted_style(),
    )]));
    render_clipped_lines(area, buffer, lines);
}

fn render_proxy_popup(bounds: Rect, proxy_area: Rect, buffer: &mut Buffer, app: &TuiApp) {
    match app.dashboard_proxy_popup {
        DashboardProxyPopup::None => {}
        DashboardProxyPopup::Groups => render_group_popup(bounds, proxy_area, buffer, app),
        DashboardProxyPopup::Nodes => render_node_popup(bounds, proxy_area, buffer, app),
    }
}

fn render_group_popup(bounds: Rect, proxy_area: Rect, buffer: &mut Buffer, app: &TuiApp) {
    let popup = popup_rect(bounds, proxy_area, 14);
    super::layout::clear_area(popup, buffer);
    let block = themed_block("选择代理组");
    let inner = block.inner(popup);
    block.render(popup, buffer);
    let width = usize::from(inner.width).max(1);
    let indices = app.filtered_dashboard_proxy_group_indices();
    let row_count = popup_row_count(inner, 3);
    let (visible, offset) = visible_indices_with_offset(&indices, app.dashboard_proxy_group_index, row_count);

    Paragraph::new(Line::from(fit_display_width(
        &format!(
            "选择代理组  {}/{}",
            selected_position(&indices, app.dashboard_proxy_group_index),
            indices.len().max(app.proxy_groups.len())
        ),
        width,
    )))
    .render(Rect::new(inner.x, inner.y, inner.width, 1), buffer);
    let group_widths = [
        Constraint::Length(2),
        Constraint::Length(20),
        Constraint::Min(12),
        Constraint::Length(6),
    ];
    render_popup_table_header(
        Rect::new(inner.x, inner.y.saturating_add(1), inner.width, 1),
        buffer,
        &group_widths,
        Row::new(["", "组", "当前节点", "节点"]),
    );

    let list_area = Rect::new(inner.x, inner.y.saturating_add(2), inner.width, row_count as u16);
    let rows = visible
        .iter()
        .filter_map(|index| {
            let group = app.proxy_groups.get(*index)?;
            let marker = if *index == app.dashboard_proxy_group_index {
                ">"
            } else {
                " "
            };
            Some(Row::new(vec![
                Cell::from(marker.to_owned()),
                Cell::from(table_cell_text(&group.name)),
                Cell::from(table_cell_text(&group.now)),
                Cell::from(group.nodes.len().to_string()),
            ]))
        })
        .collect::<Vec<_>>();
    let selected = visible
        .iter()
        .position(|index| *index == app.dashboard_proxy_group_index);
    render_popup_table(list_area, buffer, &group_widths, rows, selected);
    render_vertical_scrollbar(
        popup_scrollbar_area(popup, list_area),
        buffer,
        indices.len(),
        row_count,
        offset,
    );

    let footer_y = list_area.y.saturating_add(list_area.height);
    Paragraph::new(Line::from(Span::styled(
        display_text("Enter 定位节点；Esc 收起。"),
        muted_style(),
    )))
    .render(
        Rect::new(inner.x, footer_y, inner.width, inner.bottom().saturating_sub(footer_y)),
        buffer,
    );
}

fn render_node_popup(bounds: Rect, proxy_area: Rect, buffer: &mut Buffer, app: &TuiApp) {
    let popup = popup_rect(bounds, proxy_area, 16);
    super::layout::clear_area(popup, buffer);
    let block = themed_block("选择节点");
    let inner = block.inner(popup);
    block.render(popup, buffer);
    let width = usize::from(inner.width).max(1);
    let Some(group) = app.dashboard_proxy_group() else {
        Paragraph::new(display_line("未选择代理组")).render(inner, buffer);
        return;
    };
    let indices = app.filtered_dashboard_proxy_node_indices();
    let row_count = popup_row_count(inner, 3);
    let (visible, offset) = visible_indices_with_offset(&indices, app.dashboard_proxy_node_index, row_count);

    Paragraph::new(Line::from(fit_display_width(
        &format!(
            "{} / 选择节点  {}/{}",
            table_text(&group.name, 18),
            selected_position(&indices, app.dashboard_proxy_node_index),
            group.nodes.len()
        ),
        width,
    )))
    .render(Rect::new(inner.x, inner.y, inner.width, 1), buffer);
    let delay_width = 12;
    let node_widths = [
        Constraint::Length(2),
        Constraint::Min(8),
        Constraint::Length(delay_width as u16),
    ];
    render_popup_table_header(
        Rect::new(inner.x, inner.y.saturating_add(1), inner.width, 1),
        buffer,
        &node_widths,
        Row::new(["", "节点", "延迟"]),
    );

    let list_area = Rect::new(inner.x, inner.y.saturating_add(2), inner.width, row_count as u16);
    let rows = visible
        .iter()
        .filter_map(|index| {
            let node = group.nodes.get(*index)?;
            let marker = if *index == app.dashboard_proxy_node_index {
                ">"
            } else {
                " "
            };
            let delay_ms = app.proxy_node_meta.get(node).and_then(|meta| meta.delay_ms);
            let delay = format_proxy_delay(delay_ms);
            Some(Row::new(vec![
                Cell::from(marker.to_owned()),
                Cell::from(table_cell_text(node)),
                Cell::from(table_cell_text(&delay)).style(proxy_delay_style(delay_ms)),
            ]))
        })
        .collect::<Vec<_>>();
    let selected = visible
        .iter()
        .position(|index| *index == app.dashboard_proxy_node_index);
    render_popup_table(list_area, buffer, &node_widths, rows, selected);
    render_vertical_scrollbar(
        popup_scrollbar_area(popup, list_area),
        buffer,
        indices.len(),
        row_count,
        offset,
    );

    let action = if group.offline { "预选节点" } else { "应用节点" };
    let footer_y = list_area.y.saturating_add(list_area.height);
    Paragraph::new(Line::from(Span::styled(
        display_text(&format!("Enter {action}；Esc 收起。")),
        muted_style(),
    )))
    .render(
        Rect::new(inner.x, footer_y, inner.width, inner.bottom().saturating_sub(footer_y)),
        buffer,
    );
}

fn popup_rect(bounds: Rect, proxy_area: Rect, desired_height: u16) -> Rect {
    let min_width = 36;
    let min_height = 7;
    let x = proxy_area.x.saturating_sub(1).max(bounds.x.saturating_add(1));
    let max_right = bounds.right();
    let available_width = max_right.saturating_sub(x);
    let width = if available_width >= min_width {
        available_width
    } else {
        available_width.max(1)
    };

    let preferred_y = proxy_area.y.saturating_add(5);
    let max_bottom = bounds.bottom();
    let y = preferred_y.min(max_bottom.saturating_sub(min_height));
    let available_height = max_bottom.saturating_sub(y).max(1);
    let height = desired_height
        .min(available_height)
        .max(available_height.min(min_height));

    Rect::new(x, y, width, height)
}

const fn popup_scrollbar_area(popup: Rect, list_area: Rect) -> Rect {
    Rect::new(popup.x, list_area.y, popup.width, list_area.height)
}

fn popup_row_count(inner: Rect, fixed_lines: u16) -> usize {
    usize::from(inner.height.saturating_sub(fixed_lines)).max(1)
}

fn render_clipped_lines(area: Rect, buffer: &mut Buffer, lines: Vec<Line<'static>>) {
    let max = usize::from(area.height).max(1);
    let lines = lines.into_iter().take(max).collect::<Vec<_>>();
    Paragraph::new(lines).render(area, buffer);
}

fn terminal_type_label() -> String {
    let term = env_value("TERM");
    let mut parts = Vec::new();
    if let Some(program) = terminal_program_label() {
        parts.push(program);
        if let Some(term) = term.as_deref().filter(|term| !term.trim().is_empty()) {
            parts.push(term.to_owned());
        }
    } else if let Some(term) = term {
        parts.push(term);
    } else {
        parts.push("未知".to_owned());
    }

    if env_value("SSH_TTY").is_some() || env_value("SSH_CONNECTION").is_some() {
        parts.push("SSH".to_owned());
    }
    if env_value("TMUX").is_some() {
        parts.push("tmux".to_owned());
    }
    if env_value("STY").is_some() {
        parts.push("screen".to_owned());
    }
    if let Some(color) = env_value("COLORTERM") {
        parts.push(color);
    }

    parts.join(" / ")
}

fn terminal_program_label() -> Option<String> {
    if let Some(program) = env_value("TERM_PROGRAM") {
        return Some(match env_value("TERM_PROGRAM_VERSION") {
            Some(version) => format!("{program} {version}"),
            None => program,
        });
    }
    if env_value("WT_SESSION").is_some() {
        return Some("Windows Terminal".to_owned());
    }
    if env_value("KONSOLE_VERSION").is_some() {
        return Some("Konsole".to_owned());
    }
    if env_value("VTE_VERSION").is_some() {
        return Some("VTE".to_owned());
    }
    None
}

fn env_value(key: &str) -> Option<String> {
    env::var(key).ok().filter(|value| !value.trim().is_empty())
}

fn key_value_line(label: &str, value: &str, tail: Option<String>, width: usize) -> Line<'static> {
    let label = pad_right_display(label, LABEL_WIDTH);
    let value_width = width.saturating_sub(LABEL_WIDTH + 2);
    let mut line = format!("{label}{value}");
    if let Some(tail) = tail.filter(|tail| !tail.trim().is_empty()) {
        let used = display_width(&line);
        let tail_width = display_width(&tail);
        if used + tail_width + 2 < width {
            line.push_str(&" ".repeat(width - used - tail_width));
            line.push_str(&tail);
        } else {
            line = fit_display_width(&format!("{line}  {tail}"), width);
        }
    }
    let style = if value.contains("运行中") || value.contains("开启") {
        ok_style()
    } else {
        panel_style()
    };
    Line::from(Span::styled(
        fit_display_width(&line, value_width + LABEL_WIDTH + 2),
        style,
    ))
}

fn key_value_line_with_tail_spans(
    label: &str,
    value: &str,
    tail_spans: Vec<Span<'static>>,
    width: usize,
) -> Line<'static> {
    let label = pad_right_display(label, LABEL_WIDTH);
    let tail_width = Line::from(tail_spans.clone()).width();
    let value_width = width.saturating_sub(LABEL_WIDTH + tail_width + 1).max(1);
    let value = fit_display_width(value, value_width);
    let used = display_width(&label) + display_width(&value);
    let spaces = width.saturating_sub(used + tail_width).max(1);
    let mut spans = vec![
        Span::styled(label, panel_style()),
        Span::styled(value, panel_style()),
        Span::styled(" ".repeat(spaces), panel_style()),
    ];
    spans.extend(tail_spans);
    Line::from(spans)
}

fn proxy_status_tail_spans(
    offline: bool,
    delay_ms: Option<i64>,
    alive: Option<bool>,
    delay: &str,
    alive_label: &str,
) -> Vec<Span<'static>> {
    if offline {
        return vec![Span::styled(display_text(alive_label), muted_style())];
    }
    vec![
        Span::styled(display_text(delay), proxy_delay_style(delay_ms)),
        Span::styled(" ", panel_style()),
        Span::styled(display_text(alive_label), alive_status_style(alive)),
    ]
}

fn alive_status_style(alive: Option<bool>) -> ratatui::style::Style {
    match alive {
        Some(true) => ok_style(),
        Some(false) => warn_style(),
        None => muted_style(),
    }
}

fn render_popup_table_header<const N: usize>(
    area: Rect,
    buffer: &mut Buffer,
    widths: &[Constraint; N],
    header: Row<'static>,
) {
    Widget::render(Table::new([header], *widths), area, buffer);
}

fn render_popup_table<const N: usize>(
    area: Rect,
    buffer: &mut Buffer,
    widths: &[Constraint; N],
    rows: Vec<Row<'static>>,
    selected: Option<usize>,
) {
    let table = Table::new(rows, *widths).row_highlight_style(selected_style());
    let mut state = TableState::default().with_selected(selected);
    StatefulWidget::render(table, area, buffer, &mut state);
}

fn progress_line(label: &str, percent: Option<f64>, width: usize) -> Line<'static> {
    let label = pad_right_display(label, LABEL_WIDTH);
    let bar_width = width.saturating_sub(LABEL_WIDTH + 10).clamp(8, 32);
    let filled = percent
        .map(|percent| ((percent / 100.0) * bar_width as f64).round() as usize)
        .unwrap_or(0)
        .min(bar_width);
    let empty = bar_width.saturating_sub(filled);
    let percent_label = percent.map_or_else(|| "未知".into(), |percent| format!("{percent:.1}%"));
    let (filled_symbol, empty_symbol) = progress_symbols();
    Line::from(vec![
        Span::raw(label),
        Span::styled(filled_symbol.repeat(filled), warn_style()),
        Span::styled(empty_symbol.repeat(empty), progress_empty_style()),
        Span::raw(format!("  {percent_label}")),
    ])
}

fn switch_table_row(name: &str, state: &str, key: &str, description: &str) -> Row<'static> {
    Row::new(vec![
        Cell::from(table_cell_text(name)),
        Cell::from(table_cell_text(state)),
        Cell::from(table_cell_text(key)),
        Cell::from(table_cell_text(description)),
    ])
}

fn core_summary(app: &TuiApp) -> (String, String) {
    let Some(snapshot) = app.kernel_snapshot.as_ref() else {
        return ("Mihomo Meta 未刷新 / PID -".into(), "未刷新".into());
    };
    let version = snapshot.version.as_deref().unwrap_or("未知版本");
    let pid = snapshot.pid.map_or_else(|| "-".into(), |pid| pid.to_string());
    (
        format!("{version} / PID {pid}"),
        kernel_state_label(snapshot.state).to_owned(),
    )
}

fn core_owner_summary(app: &TuiApp) -> (String, Option<String>) {
    let Some(snapshot) = app.kernel_snapshot.as_ref() else {
        return ("未刷新".into(), None);
    };
    let detail = snapshot
        .owner_detail
        .as_deref()
        .filter(|detail| !detail.trim().is_empty())
        .map(str::to_owned);
    (kernel_owner_label(snapshot.owner).to_owned(), detail)
}

const fn bracket_bool(enabled: bool) -> &'static str {
    if enabled { "[开启]" } else { "[关闭]" }
}

fn format_speed(value: Option<u64>) -> String {
    value.map_or_else(|| "-".into(), |bytes| format!("{}/s", format_bytes(bytes)))
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

struct SubscriptionUsage {
    profile: Option<String>,
    summary: String,
    percent: Option<f64>,
    updated: String,
}

fn subscription_usage(app: &TuiApp) -> SubscriptionUsage {
    let Some(profile) = current_profile(app) else {
        return SubscriptionUsage {
            profile: None,
            summary: "未选择订阅".into(),
            percent: None,
            updated: "无".into(),
        };
    };
    let profile_label = profile
        .name
        .as_deref()
        .or(profile.uid.as_deref())
        .map(|name| table_text(name, 18));
    let Some(extra) = profile.extra else {
        return SubscriptionUsage {
            profile: profile_label,
            summary: "订阅未返回流量".into(),
            percent: None,
            updated: updated_label(profile.updated),
        };
    };
    let used = extra.upload.saturating_add(extra.download);
    let percent = (extra.total > 0).then(|| (used as f64 / extra.total as f64 * 100.0).clamp(0.0, 999.9));
    let total = if extra.total > 0 {
        format_bytes(extra.total)
    } else {
        "未知".into()
    };
    SubscriptionUsage {
        profile: profile_label,
        summary: format!("已用 {} / 总量 {total}", format_bytes(used)),
        percent,
        updated: updated_label(profile.updated),
    }
}

fn current_profile(app: &TuiApp) -> Option<&clash_core::PrfItem> {
    let current = app.profiles_current.as_deref()?;
    app.profiles
        .iter()
        .find(|profile| profile.uid.as_deref() == Some(current))
}

fn updated_label(updated: Option<usize>) -> String {
    let Some(updated) = updated.and_then(|value| u64::try_from(value).ok()) else {
        return "未更新".into();
    };
    let now = current_timestamp_secs();
    if updated > now {
        return "刚刚".into();
    }
    let elapsed = now.saturating_sub(updated);
    match elapsed {
        0..=59 => "刚刚".into(),
        60..=3599 => format!("{}分钟前", elapsed.div_ceil(60)),
        3600..=86_399 => format!("{}小时前", elapsed.div_ceil(3600)),
        _ => format!("{}天前", elapsed.div_ceil(86_400)),
    }
}

fn current_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn selected_position(indices: &[usize], selected: usize) -> usize {
    indices
        .iter()
        .position(|index| *index == selected)
        .map_or(0, |index| index + 1)
}
