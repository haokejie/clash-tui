use ratatui::{
    layout::Constraint,
    layout::Rect,
    text::Line,
    widgets::{Cell, Paragraph, Row, Table, Widget as _},
};

use crate::tui::{TuiApp, bool_label, content_rows, terminal_safe_log_text, visible_indices};

use super::layout::{display_line, table_cell_text};

pub fn render(area: Rect, buffer: &mut ratatui::buffer::Buffer, app: &TuiApp) {
    let indices = app.filtered_log_indices();
    let summary = LogSummary::from_logs(&app.logs);
    let core_log_enabled = app
        .settings
        .as_ref()
        .map(|settings| settings.core_log_enabled)
        .unwrap_or(true);
    let lines = vec![
        display_line(format!(
            "日志：{} | 显示：{} | 错误：{} | 警告：{} | 核心日志：{} | 等级：{} | 跟随：{}",
            app.logs.len(),
            indices.len(),
            summary.errors,
            summary.warnings,
            bool_label(core_log_enabled),
            app.log_level_filter.title(),
            bool_label(app.log_follow)
        )),
        display_line("操作：↑↓滚动 | PgUp/PgDn 翻页 | Enter 详情 | f 跟随 | L 等级 | / 过滤 | x 清屏"),
        Line::from(""),
    ];
    let block = super::layout::themed_block("日志");
    let inner = block.inner(area);
    block.render(area, buffer);
    Paragraph::new(lines).render(Rect::new(inner.x, inner.y, inner.width, 3.min(inner.height)), buffer);

    let max_rows = content_rows(area, 3);
    if indices.is_empty() {
        let empty_message = if !core_log_enabled {
            "核心日志已关闭；旧日志可清空，重新开启并重启 Core 后继续记录"
        } else {
            "没有匹配当前过滤条件的核心日志"
        };
        Paragraph::new(display_line(empty_message)).render(
            Rect::new(
                inner.x,
                inner.y.saturating_add(3),
                inner.width,
                inner.height.saturating_sub(3),
            ),
            buffer,
        );
        return;
    }

    let rows = visible_indices(&indices, app.log_index, max_rows)
        .iter()
        .filter_map(|index| {
            let log = app.logs.get(*index)?;
            let marker = if *index == app.log_index { ">" } else { " " };
            let row = LogRow::from_log(log);
            Some(Row::new(vec![
                Cell::from(marker),
                Cell::from(table_cell_text(row.level)),
                Cell::from(table_cell_text(&row.time)),
                Cell::from(table_cell_text(&row.message)),
            ]))
        });
    Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Length(8),
            Constraint::Length(22),
            Constraint::Min(24),
        ],
    )
    .header(Row::new(vec!["", "等级", "时间", "摘要"]))
    .render(
        Rect::new(
            inner.x,
            inner.y.saturating_add(3),
            inner.width,
            inner.height.saturating_sub(3),
        ),
        buffer,
    );
}

#[derive(Debug, Clone, Copy, Default)]
struct LogSummary {
    errors: usize,
    warnings: usize,
}

impl LogSummary {
    fn from_logs(logs: &[String]) -> Self {
        let mut summary = Self::default();
        for log in logs {
            match log_level(log) {
                "错误" | "致命" => summary.errors += 1,
                "警告" => summary.warnings += 1,
                _ => {}
            }
        }
        summary
    }
}

#[derive(Debug, Clone)]
struct LogRow {
    level: &'static str,
    time: String,
    message: String,
}

impl LogRow {
    fn from_log(log: &str) -> Self {
        let safe = terminal_safe_log_text(log);
        Self {
            level: log_level(&safe),
            time: log_field(&safe, "time").unwrap_or_else(|| "-".into()),
            message: log_field(&safe, "msg").unwrap_or(safe),
        }
    }
}

fn log_level(log: &str) -> &'static str {
    let lower = log.to_ascii_lowercase();
    if lower.contains("level=fatal")
        || lower.contains("level=\"fatal\"")
        || lower.contains("[fatal]")
        || lower.contains("fatal:")
    {
        "致命"
    } else if lower.contains("level=error")
        || lower.contains("level=\"error\"")
        || lower.contains("[error]")
        || lower.contains("error:")
    {
        "错误"
    } else if lower.contains("level=warn")
        || lower.contains("level=\"warn\"")
        || lower.contains("[warn]")
        || lower.contains("warning:")
    {
        "警告"
    } else if lower.contains("level=debug")
        || lower.contains("level=\"debug\"")
        || lower.contains("level=trace")
        || lower.contains("level=\"trace\"")
        || lower.contains("[debug]")
        || lower.contains("[trace]")
    {
        "调试"
    } else if lower.contains("level=info") || lower.contains("level=\"info\"") || lower.contains("[info]") {
        "信息"
    } else {
        "其他"
    }
}

fn log_field(log: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    let start = log.find(&prefix)? + prefix.len();
    let value = &log[start..];
    if let Some(stripped) = value.strip_prefix('"') {
        let end = stripped.find('"').unwrap_or(stripped.len());
        return Some(stripped[..end].to_owned());
    }
    let end = value.find(char::is_whitespace).unwrap_or(value.len());
    let value = value[..end].trim();
    (!value.is_empty()).then(|| value.to_owned())
}
