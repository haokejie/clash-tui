use tokio::sync::broadcast::error::TryRecvError;

use crate::jobs::{ClashTuiEventPayload, JobRecord, JobStatus};

use super::{
    labels::{job_status_label, kernel_state_label},
    state::TuiApp,
};

pub(crate) fn drain_job_events(
    app: &mut TuiApp,
    events: &mut tokio::sync::broadcast::Receiver<crate::jobs::ClashTuiEvent>,
) {
    loop {
        match events.try_recv() {
            Ok(event) => match event.payload {
                ClashTuiEventPayload::JobCreated { job } => {
                    app.set_refresh_status(format!("任务已创建：{}", job.name));
                }
                ClashTuiEventPayload::JobUpdated { job } => {
                    if current_profile_update_job_succeeded(app, &job) {
                        app.clear_profile_bound_runtime_state();
                        app.last_refresh = None;
                        app.set_refresh_status(format!(
                            "任务{}：{}；当前订阅已更新，正在刷新代理列表",
                            job_status_label(job.status),
                            job.name
                        ));
                    } else {
                        app.set_refresh_status(format!("任务{}：{}", job_status_label(job.status), job.name));
                    }
                }
                ClashTuiEventPayload::KernelStateChanged { kernel } => {
                    app.set_refresh_status(format!("核心状态：{}", kernel_state_label(kernel.state)));
                }
                ClashTuiEventPayload::MihomoTraffic { .. } | ClashTuiEventPayload::MihomoLog { .. } => {}
            },
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Lagged(_)) => {
                app.set_refresh_status("事件流滞后，已刷新快照");
                break;
            }
            Err(TryRecvError::Closed) => break,
        }
    }
}

pub(crate) fn current_profile_update_job_succeeded(app: &TuiApp, job: &JobRecord) -> bool {
    if job.kind != "profile-update" || job.status != JobStatus::Succeeded {
        return false;
    }
    let Some(target) = job.target.as_deref() else {
        return false;
    };
    let result_current = job
        .result
        .as_ref()
        .and_then(|result| result.get("current"))
        .and_then(serde_json::Value::as_str);
    result_current == Some(target) || app.profiles_current.as_deref() == Some(target)
}
