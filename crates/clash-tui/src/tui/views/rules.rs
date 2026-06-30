use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    text::Line,
    widgets::{Cell, Paragraph, Row, StatefulWidget, Table, TableState, Widget as _},
};

use crate::tui::{TuiApp, content_rows, visible_indices};

use super::layout::{fit_display_width, selected_style, table_cell_text};

const RULE_TYPE_WIDTH: usize = 16;
const RULE_PAYLOAD_WIDTH: usize = 44;

pub fn render(area: Rect, buffer: &mut ratatui::buffer::Buffer, app: &TuiApp) {
    let indices = app.filtered_rule_indices();
    let content_width = usize::from(area.width.saturating_sub(4)).max(32);
    let action_line = if app.rule_providers.is_empty() {
        "操作：↑↓查看 | / 按类型、内容、代理过滤".to_owned()
    } else {
        "操作：↑↓查看 | p 规则 Provider | 弹窗内 u/a 更新规则 | / 过滤".to_owned()
    };
    let lines = vec![
        Line::from(fit_display_width(
            &format!("规则：{} | 显示：{}", app.rules.len(), indices.len()),
            content_width,
        )),
        Line::from(fit_display_width(&action_line, content_width)),
        Line::from(""),
    ];

    let block = super::layout::themed_block("规则");
    let inner = block.inner(area);
    block.render(area, buffer);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(lines.len() as u16), Constraint::Min(1)])
        .split(inner);
    Paragraph::new(lines).render(chunks[0], buffer);

    if indices.is_empty() {
        Paragraph::new("没有匹配当前过滤条件的规则").render(chunks[1], buffer);
        return;
    }

    let max_rows = content_rows(area, 4);
    let visible = visible_indices(&indices, app.rule_index, max_rows);
    let selected = visible.iter().position(|index| *index == app.rule_index);
    let rows = visible.iter().filter_map(|index| {
        let rule = app.rules.get(*index)?;
        let marker = if *index == app.rule_index { ">" } else { " " };
        Some(Row::new(vec![
            Cell::from(marker),
            Cell::from(table_cell_text(rule.r#type.as_deref().unwrap_or("RULE"))),
            Cell::from(table_cell_text(rule.payload.as_deref().unwrap_or("-"))),
            Cell::from(table_cell_text(rule.proxy.as_deref().unwrap_or("-"))),
        ]))
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Length(RULE_TYPE_WIDTH as u16),
            Constraint::Length(RULE_PAYLOAD_WIDTH as u16),
            Constraint::Min(12),
        ],
    )
    .header(Row::new(vec!["", "类型", "内容", "代理"]))
    .row_highlight_style(selected_style());
    let mut state = TableState::default().with_selected(selected);
    StatefulWidget::render(table, chunks[1], buffer, &mut state);
}
