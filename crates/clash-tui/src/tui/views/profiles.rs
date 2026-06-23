use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    text::Line,
    widgets::{Cell, Paragraph, Row, StatefulWidget, Table, TableState, Widget as _},
};

use crate::{
    subscriptions::SubscriptionProfileStatus,
    tui::{
        TuiApp, content_rows, job_status_label, profile_update_message_label, sanitize_url_error, seconds_until_label,
        visible_indices,
    },
};

use super::layout::{display_line, fit_display_width, selected_style, table_cell_text, themed_block};

const PROFILE_NAME_WIDTH: usize = 24;
const PROFILE_KIND_WIDTH: usize = 8;
const PROFILE_UID_WIDTH: usize = 16;

pub fn render(area: Rect, buffer: &mut ratatui::buffer::Buffer, app: &TuiApp) {
    let indices = app.filtered_profile_indices();
    let mut lines = Vec::new();
    let content_width = usize::from(area.width.saturating_sub(4)).max(32);
    let now_secs = current_timestamp_secs();

    lines.push(Line::from(fit_display_width(
        &format!(
            "当前：{} | 订阅：{} | 显示：{}",
            app.profiles_current.as_deref().unwrap_or("无"),
            app.profiles.len(),
            indices.len()
        ),
        content_width,
    )));
    if let Some(status) = app.subscription_status.as_ref() {
        let remote = status.profiles.iter().filter(|profile| profile.remote).count();
        let due = status.profiles.iter().filter(|profile| profile.due).count();
        let failed = status
            .profiles
            .iter()
            .filter(|profile| profile.latest_failure.is_some())
            .count();
        let active = status
            .profiles
            .iter()
            .filter(|profile| profile.active_job.is_some())
            .count();
        let next = next_profile_update_label(&status.profiles, now_secs);
        lines.push(Line::from(fit_display_width(
            &format!("订阅检查：远程={remote} 到期={due} 进行中={active} 下次到期={next} 失败={failed}"),
            content_width,
        )));
    } else {
        lines.push(display_line("订阅检查：尚未刷新"));
    }
    lines.push(Line::from(fit_display_width(
        "操作：直接粘贴订阅链接 | Enter 切换 | i 输入URL | o 本地导入 | u 更新选中 | a 更新全部 | d 删除 | / 过滤",
        content_width,
    )));
    lines.push(Line::from(""));

    let block = themed_block("订阅");
    let inner = block.inner(area);
    block.render(area, buffer);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(lines.len() as u16), Constraint::Min(1)])
        .split(inner);
    Paragraph::new(lines).render(chunks[0], buffer);

    if indices.is_empty() {
        Paragraph::new("没有匹配当前过滤条件的订阅").render(chunks[1], buffer);
        return;
    }

    let max_rows = content_rows(area, 5);
    let visible = visible_indices(&indices, app.profile_index, max_rows);
    let selected = visible.iter().position(|index| *index == app.profile_index);
    let rows = visible.iter().filter_map(|index| {
        let profile = app.profiles.get(*index)?;
        let marker = if *index == app.profile_index { ">" } else { " " };
        let current = if app.profiles_current.as_deref() == profile.uid.as_deref() {
            "*"
        } else {
            " "
        };
        let kind = profile_kind_label(profile.itype.as_deref());
        let uid = profile.uid.as_deref().unwrap_or("-");
        let name = profile.name.as_deref().unwrap_or(uid);
        let subscription = app
            .subscription_status
            .as_ref()
            .and_then(|status| {
                status
                    .profiles
                    .iter()
                    .find(|item| item.uid.as_deref() == profile.uid.as_deref())
            })
            .map(|status| subscription_status_label(status, now_secs))
            .unwrap_or_else(|| "-".into());
        Some(Row::new(vec![
            Cell::from(marker),
            Cell::from(current),
            Cell::from(table_cell_text(&sanitize_url_error(name))),
            Cell::from(table_cell_text(kind)),
            Cell::from(table_cell_text(uid)),
            Cell::from(table_cell_text(&subscription)),
        ]))
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(PROFILE_NAME_WIDTH as u16),
            Constraint::Length(PROFILE_KIND_WIDTH as u16),
            Constraint::Length(PROFILE_UID_WIDTH as u16),
            Constraint::Min(12),
        ],
    )
    .header(Row::new(vec!["", "当前", "名称", "类型", "UID", "状态"]))
    .row_highlight_style(selected_style());
    let mut state = TableState::default().with_selected(selected);
    StatefulWidget::render(table, chunks[1], buffer, &mut state);
}

pub(crate) fn subscription_status_label(status: &SubscriptionProfileStatus, now_secs: u64) -> String {
    if let Some(job) = status.active_job.as_ref() {
        format!("任务{}", job_status_label(job.status))
    } else if let Some(failure) = status.latest_failure.as_ref() {
        format!("失败：{}", sanitize_url_error(failure))
    } else if status.due {
        "到期".to_owned()
    } else if let Some(result) = status.latest_result.as_ref() {
        subscription_result_label(result)
    } else if status.due_reason == "scheduled" {
        status.next_update_at.map_or_else(
            || due_reason_label(&status.due_reason).to_owned(),
            |next| format!("已排期：{}", timestamp_until_label(next, now_secs)),
        )
    } else {
        due_reason_label(&status.due_reason).to_owned()
    }
}

pub(crate) fn next_profile_update_label(statuses: &[SubscriptionProfileStatus], now_secs: u64) -> String {
    statuses
        .iter()
        .filter(|profile| profile.remote && profile.auto_update_enabled)
        .filter_map(|profile| profile.next_update_at)
        .min()
        .map_or_else(|| "无".into(), |next| timestamp_until_label(next, now_secs))
}

fn timestamp_until_label(timestamp_secs: u64, now_secs: u64) -> String {
    if timestamp_secs <= now_secs {
        "已到期".into()
    } else {
        seconds_until_label(timestamp_secs - now_secs)
    }
}

fn subscription_result_label(result: &str) -> String {
    let result = profile_update_message_label(result);
    if result.trim().is_empty() || result.trim() == "订阅已更新" {
        "最近成功".to_owned()
    } else {
        format!("成功：{result}")
    }
}

fn current_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn profile_kind_label(kind: Option<&str>) -> &'static str {
    match kind {
        Some("local") => "本地",
        Some("remote") => "远程",
        Some("merge") => "合并",
        Some("script") => "脚本",
        Some("rules") => "规则",
        _ => "-",
    }
}

fn due_reason_label(reason: &str) -> &'static str {
    match reason {
        "not a remote subscription" => "非远程订阅",
        "auto update disabled" => "到期检查关闭",
        "missing update interval" => "缺少更新间隔",
        "update interval is zero" => "更新间隔为 0",
        "missing last update timestamp" => "缺少上次更新时间",
        "due now" => "已到期",
        "scheduled" => "已排期",
        _ => "未知状态",
    }
}
