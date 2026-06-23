use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{Cell, Paragraph, Row, StatefulWidget, Table, TableState, Widget},
};

use crate::{
    state::AppState,
    tui::{ProxyPane, TuiApp, kernel_state_label, mode_label, visible_indices, visible_indices_with_offset},
};

use super::layout::{
    border_style, fit_display_width, format_proxy_delay, horizontal_symbol, muted_style, ok_style, proxy_delay_style,
    render_vertical_scrollbar, selected_style, table_cell_text, table_text, tee_bottom_symbol, tee_left_symbol,
    tee_right_symbol, tee_top_symbol, title_style, vertical_symbol, warn_style,
};

const GROUP_NAME_WIDTH: usize = 12;
const GROUP_NOW_WIDTH: usize = 12;
const GROUP_COUNT_WIDTH: usize = 6;
const NODE_DELAY_WIDTH: usize = 12;
const NODE_STATUS_WIDTH: usize = 10;
const SPLIT_MIN_WIDTH: u16 = 92;

pub fn render(area: Rect, buffer: &mut Buffer, state: &AppState, app: &TuiApp) {
    let content_width = usize::from(area.width.saturating_sub(4)).max(32);
    let mode = state.config.try_read().map_or_else(
        |_| "不可用".to_owned(),
        |config| {
            let raw_mode = config
                .clash
                .0
                .get("mode")
                .and_then(serde_yaml_ng::Value::as_str)
                .unwrap_or("rule");
            match raw_mode {
                "global" => mode_label(crate::mihomo_controller::Mode::Global).to_owned(),
                "direct" => mode_label(crate::mihomo_controller::Mode::Direct).to_owned(),
                _ => mode_label(crate::mihomo_controller::Mode::Rule).to_owned(),
            }
        },
    );

    let core = app
        .kernel_snapshot
        .as_ref()
        .map_or("未刷新", |snapshot| kernel_state_label(snapshot.state));
    let offline_status = if app.proxy_groups.iter().any(|group| group.offline) {
        " | 状态：runtime预选，可预选"
    } else {
        ""
    };
    let provider_hint = if app.proxy_providers.is_empty() {
        ""
    } else {
        " | p Provider"
    };
    let header_lines = vec![
        Line::from(fit_display_width(
            &format!(
                "模式：{mode}{offline_status} | 核心：{core} | {} | 焦点：{}",
                app.controller_status,
                app.proxy_pane.title()
            ),
            content_width,
        )),
        Line::from(fit_display_width(
            &format!("操作：Enter 定位/应用节点 | f 切焦点 | t 测速 | S 节点排序{provider_hint} | m 模式 | / 过滤"),
            content_width,
        )),
    ];

    let block = super::layout::themed_block("代理");
    let inner = block.inner(area);
    block.render(area, buffer);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(1)])
        .split(inner);
    Paragraph::new(header_lines).render(chunks[0], buffer);

    if chunks[1].width < SPLIT_MIN_WIDTH {
        render_narrow_fallback(chunks[1], buffer, app, content_width);
        return;
    }

    render_split_proxy_page(chunks[1], buffer, app);
}

fn render_split_proxy_page(area: Rect, buffer: &mut Buffer, app: &TuiApp) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);
    let title_line = split_title_line_rect(sections[0]);
    let body = sections[1];
    draw_title_separator_line(title_line, buffer);

    if app.proxy_groups.is_empty() {
        render_empty_proxy_state(body, buffer, app);
        return;
    }

    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30),
            Constraint::Length(1),
            Constraint::Percentage(70),
        ])
        .split(body);
    let groups_area = main_chunks[0];
    let divider = main_chunks[1];
    let nodes_area = main_chunks[2];

    draw_split_title_line(title_line, divider, buffer, "代理组", "节点");
    draw_vertical_line(divider, buffer);

    render_group_panel(groups_area, buffer, app);
    render_node_panel(nodes_area, buffer, app);
    draw_split_bottom_joint(body, divider, buffer);
}

fn render_group_panel(area: Rect, buffer: &mut Buffer, app: &TuiApp) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let indices = app.filtered_proxy_group_indices();
    if indices.is_empty() {
        Paragraph::new(Line::from(fit_display_width(
            "没有匹配当前过滤条件的代理组",
            usize::from(area.width),
        )))
        .render(area, buffer);
        return;
    }

    let row_count = table_row_count(area);
    let (visible, offset) = visible_indices_with_offset(&indices, app.proxy_group_index, row_count);
    let selected = visible.iter().position(|index| *index == app.proxy_group_index);
    let rows = visible
        .iter()
        .filter_map(|index| {
            let group = app.proxy_groups.get(*index)?;
            Some(Row::new(vec![
                Cell::from(if *index == app.proxy_group_index { ">" } else { " " }),
                Cell::from(table_cell_text(&group.name)),
                Cell::from(table_cell_text(&group.now)),
                Cell::from(group.nodes.len().to_string()),
            ]))
        })
        .collect::<Vec<_>>();

    let widths = [
        Constraint::Length(1),
        Constraint::Length(GROUP_NAME_WIDTH as u16),
        Constraint::Min(GROUP_NOW_WIDTH as u16),
        Constraint::Length(GROUP_COUNT_WIDTH as u16),
    ];
    render_table(
        area,
        buffer,
        widths,
        Row::new(vec!["", "组", "当前节点", "数"]),
        rows,
        selected,
        app.proxy_pane == ProxyPane::Groups,
    );
    render_vertical_scrollbar(area, buffer, indices.len(), row_count, offset);
}

fn render_node_panel(area: Rect, buffer: &mut Buffer, app: &TuiApp) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let Some(group) = app.selected_proxy_group() else {
        Paragraph::new("未选择代理组").render(area, buffer);
        return;
    };
    let indices = app.filtered_proxy_node_indices();
    if group.nodes.is_empty() || indices.is_empty() {
        let message = if group.nodes.is_empty() {
            "当前代理组没有节点"
        } else {
            "没有匹配当前过滤条件的节点"
        };
        Paragraph::new(Line::from(fit_display_width(message, usize::from(area.width)))).render(area, buffer);
        return;
    }

    let row_count = table_row_count(area);
    let (visible, offset) = visible_indices_with_offset(&indices, app.proxy_node_index, row_count);
    let selected = visible.iter().position(|index| *index == app.proxy_node_index);
    let rows = visible
        .iter()
        .filter_map(|index| {
            let node = group.nodes.get(*index)?;
            let current = if node == &group.now { "*" } else { " " };
            let meta = app.proxy_node_meta.get(node);
            let delay = format_proxy_delay(meta.and_then(|meta| meta.delay_ms));
            let alive = meta.and_then(|meta| meta.alive).map_or("未知", crate::tui::alive_label);
            let delay_style = proxy_delay_style(meta.and_then(|meta| meta.delay_ms));
            let alive_style = if meta.and_then(|meta| meta.alive).unwrap_or(false) {
                ok_style()
            } else if meta.and_then(|meta| meta.alive).is_some() {
                warn_style()
            } else {
                muted_style()
            };
            Some(Row::new(vec![
                Cell::from(if *index == app.proxy_node_index { ">" } else { " " }),
                Cell::from(current),
                Cell::from(table_cell_text(node)),
                Cell::from(table_cell_text(&delay)).style(delay_style),
                Cell::from(table_cell_text(alive)).style(alive_style),
            ]))
        })
        .collect::<Vec<_>>();

    let widths = [
        Constraint::Length(1),
        Constraint::Length(4),
        Constraint::Min(24),
        Constraint::Length(NODE_DELAY_WIDTH as u16),
        Constraint::Length(NODE_STATUS_WIDTH as u16),
    ];
    render_table(
        area,
        buffer,
        widths,
        Row::new(vec!["", "当前", "节点", "延迟", "状态"]),
        rows,
        selected,
        app.proxy_pane == ProxyPane::Nodes,
    );
    render_vertical_scrollbar(area, buffer, indices.len(), row_count, offset);
}

fn render_empty_proxy_state(area: Rect, buffer: &mut Buffer, app: &TuiApp) {
    let width = usize::from(area.width).max(1);
    let max_lines = usize::from(area.height).max(1);
    let mut lines = vec![
        Line::from(fit_display_width(
            "当前未加载策略组；按 s 启动 Core 或按 r 刷新，确认订阅已激活",
            width,
        )),
        Line::from(fit_display_width(
            "若导入后仍为空，请查看底部状态、日志页或 CLI proxy groups 对照",
            width,
        )),
    ];
    lines.push(Line::from(""));
    if let Some(summary) = app.diagnose_summary_line() {
        lines.push(Line::from(fit_display_width(&summary, width)));
        for recommendation in app.diagnose_recommendation_lines() {
            lines.push(Line::from(fit_display_width(&recommendation, width)));
        }
        for detail in app.diagnose_runtime_detail_lines() {
            lines.push(Line::from(fit_display_width(&detail, width)));
        }
    } else {
        lines.push(Line::from(fit_display_width(
            "按 D 运行诊断；退出后也可运行：clash-tui diagnose --json",
            width,
        )));
    }
    lines.truncate(max_lines);
    Paragraph::new(lines).render(area, buffer);
}

fn render_narrow_fallback(area: Rect, buffer: &mut Buffer, app: &TuiApp, width: usize) {
    let max_rows = usize::from(area.height.saturating_sub(2)).max(1);
    match app.proxy_pane {
        ProxyPane::Groups => render_narrow_groups(area, buffer, app, max_rows, width),
        ProxyPane::Nodes => render_narrow_nodes(area, buffer, app, max_rows, width),
    }
}

fn render_narrow_groups(area: Rect, buffer: &mut Buffer, app: &TuiApp, max_rows: usize, width: usize) {
    let indices = app.filtered_proxy_group_indices();
    let chunks = fallback_chunks(area);
    Paragraph::new(Line::from(fit_display_width("代理组", width))).render(chunks[0], buffer);
    if app.proxy_groups.is_empty() || indices.is_empty() {
        Paragraph::new("未加载代理组或无匹配结果").render(chunks[1], buffer);
        return;
    }
    let visible = visible_indices(&indices, app.proxy_group_index, max_rows);
    let selected = visible.iter().position(|index| *index == app.proxy_group_index);
    let rows = visible
        .iter()
        .filter_map(|index| {
            let group = app.proxy_groups.get(*index)?;
            Some(Row::new(vec![
                Cell::from(if *index == app.proxy_group_index { ">" } else { " " }),
                Cell::from(table_cell_text(&group.name)),
                Cell::from(table_cell_text(&group.now)),
                Cell::from(group.nodes.len().to_string()),
            ]))
        })
        .collect::<Vec<_>>();
    render_table(
        chunks[1],
        buffer,
        [
            Constraint::Length(1),
            Constraint::Length(GROUP_NAME_WIDTH as u16),
            Constraint::Min(12),
            Constraint::Length(GROUP_COUNT_WIDTH as u16),
        ],
        Row::new(vec!["", "组", "当前节点", "数"]),
        rows,
        selected,
        true,
    );
}

fn render_narrow_nodes(area: Rect, buffer: &mut Buffer, app: &TuiApp, max_rows: usize, width: usize) {
    let Some(group) = app.selected_proxy_group() else {
        Paragraph::new("未选择代理组").render(area, buffer);
        return;
    };
    let indices = app.filtered_proxy_node_indices();
    let chunks = fallback_chunks(area);
    Paragraph::new(Line::from(fit_display_width(
        &format!("节点：{}", table_text(&group.name, width.saturating_sub(8))),
        width,
    )))
    .render(chunks[0], buffer);
    if group.nodes.is_empty() || indices.is_empty() {
        Paragraph::new("当前代理组没有节点或无匹配结果").render(chunks[1], buffer);
        return;
    }
    let visible = visible_indices(&indices, app.proxy_node_index, max_rows);
    let selected = visible.iter().position(|index| *index == app.proxy_node_index);
    let rows = visible
        .iter()
        .filter_map(|index| {
            let node = group.nodes.get(*index)?;
            let meta = app.proxy_node_meta.get(node);
            let delay = format_proxy_delay(meta.and_then(|meta| meta.delay_ms));
            let alive = meta.and_then(|meta| meta.alive).map_or("未知", crate::tui::alive_label);
            Some(Row::new(vec![
                Cell::from(if *index == app.proxy_node_index { ">" } else { " " }),
                Cell::from(if node == &group.now { "*" } else { " " }),
                Cell::from(table_cell_text(node)),
                Cell::from(table_cell_text(&delay)).style(proxy_delay_style(meta.and_then(|meta| meta.delay_ms))),
                Cell::from(table_cell_text(alive)),
            ]))
        })
        .collect::<Vec<_>>();
    render_table(
        chunks[1],
        buffer,
        [
            Constraint::Length(1),
            Constraint::Length(4),
            Constraint::Min(18),
            Constraint::Length(NODE_DELAY_WIDTH as u16),
            Constraint::Length(NODE_STATUS_WIDTH as u16),
        ],
        Row::new(vec!["", "当前", "节点", "延迟", "状态"]),
        rows,
        selected,
        true,
    );
}

fn render_table<const N: usize>(
    area: Rect,
    buffer: &mut Buffer,
    widths: [Constraint; N],
    header: Row<'static>,
    rows: Vec<Row<'static>>,
    selected: Option<usize>,
    focused: bool,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);
    let header = header.style(muted_style().add_modifier(Modifier::BOLD));
    Widget::render(Table::new([header], widths), chunks[0], buffer);
    let highlight = if focused {
        selected_style()
    } else {
        selected_style().remove_modifier(Modifier::BOLD)
    };
    let table = Table::new(rows, widths).row_highlight_style(highlight);
    let mut state = TableState::default().with_selected(selected);
    StatefulWidget::render(table, chunks[1], buffer, &mut state);
}

fn draw_horizontal_line(area: Rect, buffer: &mut Buffer) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    for x in area.x..area.x.saturating_add(area.width) {
        buffer[(x, area.y)]
            .set_symbol(horizontal_symbol())
            .set_style(border_style());
    }
}

fn draw_title_separator_line(area: Rect, buffer: &mut Buffer) {
    draw_horizontal_line(area, buffer);
    if area.height == 0 || area.width == 0 {
        return;
    }
    buffer[(area.x, area.y)]
        .set_symbol(tee_left_symbol())
        .set_style(border_style());
    if area.width > 1 {
        let right_x = area.x.saturating_add(area.width.saturating_sub(1));
        buffer[(right_x, area.y)]
            .set_symbol(tee_right_symbol())
            .set_style(border_style());
    }
}

fn draw_vertical_line(area: Rect, buffer: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    for y in area.y..area.y.saturating_add(area.height) {
        buffer[(area.x, y)]
            .set_symbol(vertical_symbol())
            .set_style(border_style());
    }
}

const fn split_title_line_rect(inner_title_line: Rect) -> Rect {
    let left = inner_title_line.x.saturating_sub(1);
    let right = inner_title_line
        .x
        .saturating_add(inner_title_line.width)
        .saturating_add(1);
    Rect::new(
        left,
        inner_title_line.y,
        right.saturating_sub(left),
        inner_title_line.height,
    )
}

fn draw_split_bottom_joint(body: Rect, divider: Rect, buffer: &mut Buffer) {
    if body.height == 0 || divider.width == 0 {
        return;
    }
    let bottom_y = body.y.saturating_add(body.height);
    buffer[(divider.x, bottom_y)]
        .set_symbol(tee_bottom_symbol())
        .set_style(border_style());
}

fn draw_split_title_line(
    area: Rect,
    divider: Rect,
    buffer: &mut Buffer,
    left_title: &'static str,
    right_title: &'static str,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    draw_title_separator_line(area, buffer);
    draw_title_on_line(area, area.x.saturating_add(1), buffer, left_title);

    let right_edge = area.x.saturating_add(area.width);
    if divider.x >= area.x && divider.x < right_edge {
        buffer[(divider.x, area.y)]
            .set_symbol(tee_top_symbol())
            .set_style(border_style());
        draw_title_on_line(area, divider.x.saturating_add(1), buffer, right_title);
    }
}

fn draw_title_on_line(area: Rect, x: u16, buffer: &mut Buffer, title: &'static str) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let line = Line::from(Span::styled(title, title_style()));
    let title_width = line.width() as u16;
    let start = x.min(area.x.saturating_add(area.width.saturating_sub(1)));
    let width = title_width.min(area.x.saturating_add(area.width).saturating_sub(start));
    if width == 0 {
        return;
    }
    line.render(Rect::new(start, area.y, width, 1), buffer);
}

fn fallback_chunks(area: Rect) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area)
}

fn table_row_count(area: Rect) -> usize {
    usize::from(area.height.saturating_sub(1)).max(1)
}
