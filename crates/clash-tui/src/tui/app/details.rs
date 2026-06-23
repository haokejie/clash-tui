use crate::{jobs::JobRecord, mihomo_controller::ConnectionRecord, subscriptions::SubscriptionSweep};

use super::{
    labels::job_status_label,
    text::{profile_update_message_label, sanitize_url_error},
};

pub(crate) fn connection_detail_lines(connection: &ConnectionRecord) -> Vec<String> {
    let metadata = connection.metadata.as_ref();
    let chains = if connection.chains.is_empty() {
        "-".into()
    } else {
        connection.chains.join(" > ")
    };
    let mut lines = vec![
        format!("连接ID：{}", connection.id),
        format!("上传：{} bytes", connection.upload),
        format!("下载：{} bytes", connection.download),
        format!("开始时间：{}", connection.start.as_deref().unwrap_or("-")),
        format!(
            "网络：{}",
            metadata.and_then(|metadata| metadata.network.as_deref()).unwrap_or("-")
        ),
        format!(
            "类型：{}",
            metadata.and_then(|metadata| metadata.r#type.as_deref()).unwrap_or("-")
        ),
        format!(
            "源地址：{}:{}",
            metadata
                .and_then(|metadata| metadata.source_ip.as_deref())
                .unwrap_or("-"),
            metadata
                .and_then(|metadata| metadata.source_port.as_deref())
                .unwrap_or("-")
        ),
        format!(
            "目标地址：{}:{}",
            metadata
                .and_then(|metadata| metadata.destination_ip.as_deref())
                .unwrap_or("-"),
            metadata
                .and_then(|metadata| metadata.destination_port.as_deref())
                .unwrap_or("-")
        ),
        format!(
            "Host：{}",
            metadata.and_then(|metadata| metadata.host.as_deref()).unwrap_or("-")
        ),
        format!(
            "进程：{}",
            metadata.and_then(|metadata| metadata.process.as_deref()).unwrap_or("-")
        ),
        format!("规则：{}", connection.rule.as_deref().unwrap_or("-")),
        format!("规则内容：{}", connection.rule_payload.as_deref().unwrap_or("-")),
        format!("代理链：{}", sanitize_url_error(&chains)),
    ];
    if !connection.extra.is_empty() {
        lines.push("额外字段：".into());
        let pretty =
            serde_json::to_string_pretty(&connection.extra).unwrap_or_else(|err| format!("字段序列化失败：{err}"));
        let extra_lines = pretty.lines().collect::<Vec<_>>();
        for line in extra_lines.iter().take(4) {
            lines.push(format!("  {}", sanitize_url_error(line)));
        }
        if extra_lines.len() > 4 {
            lines.push("  ...".into());
        }
    }
    lines
}

pub(crate) fn job_detail_lines(job: &JobRecord) -> Vec<String> {
    let mut lines = vec![
        format!("任务ID：{}", job.id),
        format!("名称：{}", sanitize_url_error(&job.name)),
        format!("状态：{}", job_status_label(job.status)),
        format!("类型：{}", job.kind),
        format!(
            "目标：{}",
            job.target
                .as_deref()
                .map(sanitize_url_error)
                .unwrap_or_else(|| "-".into())
        ),
    ];
    if let Some(message) = job.message.as_deref().filter(|value| !value.trim().is_empty()) {
        lines.push(format!("消息：{}", profile_update_message_label(message)));
    }
    if let Some(error) = job.error.as_deref() {
        lines.push(format!("错误：{}", sanitize_url_error(error)));
    }
    lines.push(format!("创建时间戳：{}", job.created_at));
    lines.push(format!("更新时间戳：{}", job.updated_at));
    if let Some(finished_at) = job.finished_at {
        lines.push(format!("结束时间戳：{finished_at}"));
    }
    if let Some(result) = job.result.as_ref() {
        lines.push("结果：".into());
        let pretty = serde_json::to_string_pretty(result).unwrap_or_else(|err| format!("结果序列化失败：{err}"));
        let result_lines = pretty.lines().collect::<Vec<_>>();
        for line in result_lines.iter().take(6) {
            lines.push(format!("  {}", sanitize_url_error(line)));
        }
        if result_lines.len() > 6 {
            lines.push("  ...".into());
        }
    }
    lines
}

pub(crate) fn subscription_sweep_status_message(sweep: &SubscriptionSweep) -> String {
    let mut message = format!(
        "订阅批量更新：检查={} 远程={} 入队={} 已运行={}",
        sweep.checked, sweep.due, sweep.queued, sweep.skipped
    );
    if sweep.jobs.is_empty() {
        message.push_str("；没有可更新的远程订阅");
        return message;
    }

    let job_ids = sweep
        .jobs
        .iter()
        .take(3)
        .map(|job| job.id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let more = sweep.jobs.len().saturating_sub(3);
    if more > 0 {
        message.push_str(&format!("；任务={job_ids} 等 {more} 个；按 8 查看详情"));
    } else {
        message.push_str(&format!("；任务={job_ids}；按 8 查看详情"));
    }
    message
}
