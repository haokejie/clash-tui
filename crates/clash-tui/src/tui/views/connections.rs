use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    text::Line,
    widgets::{Cell, Paragraph, Row, StatefulWidget, Table, TableState, Widget as _},
};

use crate::tui::{TuiApp, content_rows, visible_indices};

use super::layout::{fit_display_width, selected_style, table_cell_text};

const CONNECTION_ID_WIDTH: usize = 12;
const CONNECTION_HOST_WIDTH: usize = 28;
const CONNECTION_PROCESS_WIDTH: usize = 16;
const CONNECTION_RULE_WIDTH: usize = 16;

pub fn render(area: Rect, buffer: &mut ratatui::buffer::Buffer, app: &TuiApp) {
    let indices = app.filtered_connection_indices();
    let content_width = usize::from(area.width.saturating_sub(4)).max(32);
    let upload_total = app.connections.iter().map(|connection| connection.upload).sum::<u64>();
    let download_total = app
        .connections
        .iter()
        .map(|connection| connection.download)
        .sum::<u64>();
    let lines = vec![
        Line::from(fit_display_width(
            &format!(
                "连接：{} | 显示：{} | 上传：{} | 下载：{}",
                app.connections.len(),
                indices.len(),
                upload_total,
                download_total
            ),
            content_width,
        )),
        Line::from(fit_display_width(
            "操作：Enter 详情弹窗 | d/Delete 关闭选中 | c 关闭全部 | / 过滤",
            content_width,
        )),
        Line::from(""),
    ];

    let block = super::layout::themed_block("连接");
    let inner = block.inner(area);
    block.render(area, buffer);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(lines.len() as u16), Constraint::Min(1)])
        .split(inner);
    Paragraph::new(lines).render(chunks[0], buffer);

    if indices.is_empty() {
        Paragraph::new("没有匹配当前过滤条件的活动连接").render(chunks[1], buffer);
        return;
    }

    let max_rows = content_rows(area, 4);
    let visible = visible_indices(&indices, app.connection_index, max_rows);
    let selected = visible.iter().position(|index| *index == app.connection_index);
    let rows = visible.iter().filter_map(|index| {
        let connection = app.connections.get(*index)?;
        let metadata = connection.metadata.as_ref();
        let host = metadata.and_then(|metadata| metadata.host.as_deref()).unwrap_or("-");
        let process = metadata.and_then(|metadata| metadata.process.as_deref()).unwrap_or("-");
        let chains = if connection.chains.is_empty() {
            "-".into()
        } else {
            connection.chains.join(" > ")
        };
        let marker = if *index == app.connection_index { ">" } else { " " };
        Some(Row::new(vec![
            Cell::from(marker),
            Cell::from(table_cell_text(&connection.id)),
            Cell::from(table_cell_text(host)),
            Cell::from(table_cell_text(process)),
            Cell::from(table_cell_text(connection.rule.as_deref().unwrap_or("-"))),
            Cell::from(table_cell_text(&chains)),
        ]))
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Length(CONNECTION_ID_WIDTH as u16),
            Constraint::Length(CONNECTION_HOST_WIDTH as u16),
            Constraint::Length(CONNECTION_PROCESS_WIDTH as u16),
            Constraint::Length(CONNECTION_RULE_WIDTH as u16),
            Constraint::Min(12),
        ],
    )
    .header(Row::new(vec!["", "连接ID", "主机/IP", "进程", "规则", "代理链"]))
    .row_highlight_style(selected_style());
    let mut state = TableState::default().with_selected(selected);
    StatefulWidget::render(table, chunks[1], buffer, &mut state);
}
