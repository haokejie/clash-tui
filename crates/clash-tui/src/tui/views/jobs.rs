use ratatui::{
    layout::{Constraint, Rect},
    text::Line,
    widgets::{Cell, Paragraph, Row, Table, Widget as _},
};

use crate::{
    jobs::JobStatus,
    tui::{TuiApp, content_rows, job_status_label, profile_update_message_label, sanitize_url_error, visible_indices},
};

use super::layout::{fit_display_width, table_cell_text};

const JOB_ID_WIDTH: usize = 22;
const JOB_STATUS_WIDTH: usize = 10;
const JOB_KIND_WIDTH: usize = 18;
const JOB_TARGET_WIDTH: usize = 18;

pub fn render(area: Rect, buffer: &mut ratatui::buffer::Buffer, app: &TuiApp) {
    let indices = app.filtered_job_indices();
    let content_width = usize::from(area.width.saturating_sub(4)).max(32);
    let summary = JobSummary::from_jobs(&app.jobs);
    let lines = vec![
        Line::from(fit_display_width(
            &format!(
                "任务：{} | 显示：{} | 等待：{} | 运行：{} | 成功：{} | 失败：{} | 取消：{} | 可重试：{}",
                app.jobs.len(),
                indices.len(),
                summary.pending,
                summary.running,
                summary.succeeded,
                summary.failed,
                summary.cancelled,
                summary.retryable
            ),
            content_width,
        )),
        Line::from(fit_display_width(&summary.focus_line(), content_width)),
        Line::from(fit_display_width(
            "操作：Enter 详情弹窗 | R 重试订阅更新 | c 取消运行中任务 | / 过滤",
            content_width,
        )),
    ];

    let block = super::layout::themed_block("任务");
    let inner = block.inner(area);
    block.render(area, buffer);
    Paragraph::new(lines).render(Rect::new(inner.x, inner.y, inner.width, 3.min(inner.height)), buffer);

    let table_area = Rect::new(
        inner.x,
        inner.y.saturating_add(3),
        inner.width,
        inner.height.saturating_sub(3),
    );
    if indices.is_empty() {
        let empty = if app.jobs.is_empty() {
            "暂无任务记录；订阅更新、批量检查或重试后会在这里显示"
        } else {
            "没有匹配当前过滤条件的任务"
        };
        Paragraph::new(empty).render(table_area, buffer);
        return;
    }

    let max_rows = content_rows(area, 4);
    let rows = visible_indices(&indices, app.job_index, max_rows)
        .iter()
        .filter_map(|index| {
            let job = app.jobs.get(*index)?;
            let marker = if *index == app.job_index { ">" } else { " " };
            Some(Row::new(vec![
                Cell::from(marker),
                Cell::from(table_cell_text(&job.id)),
                Cell::from(table_cell_text(job_status_label(job.status))),
                Cell::from(table_cell_text(&job.kind)),
                Cell::from(table_cell_text(&sanitize_url_error(
                    job.target.as_deref().unwrap_or("-"),
                ))),
                Cell::from(job.updated_at.to_string()),
                Cell::from(table_cell_text(&job_row_detail(job))),
            ]))
        });

    Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Length((JOB_ID_WIDTH + 2) as u16),
            Constraint::Length(JOB_STATUS_WIDTH as u16),
            Constraint::Length(JOB_KIND_WIDTH as u16),
            Constraint::Length(JOB_TARGET_WIDTH as u16),
            Constraint::Length(12),
            Constraint::Min(18),
        ],
    )
    .header(Row::new(vec!["", "任务ID", "状态", "类型", "目标", "更新", "摘要"]))
    .render(table_area, buffer);
}

#[derive(Debug, Clone, Default)]
struct JobSummary {
    pending: usize,
    running: usize,
    succeeded: usize,
    failed: usize,
    cancelled: usize,
    retryable: usize,
    latest_running: Option<String>,
    latest_failed: Option<String>,
}

impl JobSummary {
    fn from_jobs(jobs: &[crate::jobs::JobRecord]) -> Self {
        let mut summary = Self::default();
        for job in jobs {
            match job.status {
                JobStatus::Pending => summary.pending += 1,
                JobStatus::Running => {
                    summary.running += 1;
                    summary.latest_running = Some(job_focus_text(job));
                }
                JobStatus::Succeeded => summary.succeeded += 1,
                JobStatus::Failed => {
                    summary.failed += 1;
                    summary.latest_failed = Some(job_focus_text(job));
                    if job.kind == "profile-update" {
                        summary.retryable += 1;
                    }
                }
                JobStatus::Cancelled => summary.cancelled += 1,
            }
        }
        summary
    }

    fn focus_line(&self) -> String {
        let running = self.latest_running.as_deref().unwrap_or("无运行中任务");
        let failed = self.latest_failed.as_deref().unwrap_or("无失败任务");
        format!("关注：运行中 {running} | 最近失败 {failed}")
    }
}

fn job_focus_text(job: &crate::jobs::JobRecord) -> String {
    let target = job.target.as_deref().unwrap_or("-");
    sanitize_url_error(&format!("{} / {}", job.name, target))
}

fn job_row_detail(job: &crate::jobs::JobRecord) -> String {
    if let Some(error) = job.error.as_deref().filter(|value| !value.trim().is_empty()) {
        return format!("错误：{}", sanitize_url_error(error));
    }
    if let Some(message) = job.message.as_deref().filter(|value| !value.trim().is_empty()) {
        return profile_update_message_label(message);
    }
    match job.status {
        JobStatus::Pending => "等待调度".into(),
        JobStatus::Running => "正在执行".into(),
        JobStatus::Succeeded => "已完成".into(),
        JobStatus::Failed => "失败，Enter 查看详情或 R 重试".into(),
        JobStatus::Cancelled => "已取消".into(),
    }
}
