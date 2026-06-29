use std::fs;

use anyhow::{Context as _, Result, bail};
use clash_core::{KernelOwner, KernelSnapshot, KernelState, OperationStatus};
use tokio::{process::Command, time::sleep};

use crate::{
    actions::controller,
    kernel::{DEFAULT_SYSTEMD_SERVICE_NAME, ENV_CORE_OWNER, ENV_SERVICE_NAME},
    state::AppState,
    timeouts,
};

pub async fn status(state: &AppState) -> KernelSnapshot {
    state.kernel.external_snapshot().await
}

pub async fn logs(state: &AppState) -> Vec<String> {
    state.kernel.logs().await
}

pub async fn clear_logs(state: &AppState) -> Result<()> {
    state.kernel.clear_logs().await
}

pub async fn start(state: &AppState) -> Result<OperationStatus> {
    let snapshot = state.kernel.external_snapshot().await;
    if matches!(
        snapshot.state,
        KernelState::Running | KernelState::Starting | KernelState::Restarting | KernelState::Unhealthy
    ) && matches!(
        snapshot.owner,
        KernelOwner::Systemd | KernelOwner::Supervised | KernelOwner::External
    ) {
        return Ok(OperationStatus {
            accepted: false,
            current_job: None,
            state: snapshot.state,
            owner: snapshot.owner,
            message: Some(format!(
                "Core 已由{}管理，未重复启动",
                owner_message_suffix(snapshot.owner, snapshot.owner_detail.as_deref())
            )),
        });
    }

    if matches!(snapshot.state, KernelState::Stopped | KernelState::Crashed)
        && let Some(status) = start_systemd_service_if_available(state).await?
    {
        apply_saved_proxy_selections_if_core_active(state, &status).await;
        return Ok(status);
    }

    let status = state.kernel.start().await?;
    apply_saved_proxy_selections_if_core_active(state, &status).await;
    Ok(status)
}

pub async fn stop(state: &AppState) -> Result<OperationStatus> {
    let snapshot = state.kernel.external_snapshot().await;
    match snapshot.owner {
        KernelOwner::Systemd if is_active(snapshot.state) => delegate_systemd(state, "stop", &snapshot).await,
        KernelOwner::Supervised if is_active(snapshot.state) => {
            bail!("Core 由外部 supervisor 管理，请使用对应服务命令停止")
        }
        KernelOwner::External if is_active(snapshot.state) => {
            bail!("Core 为外部接管进程，默认不停止；请确认来源后清理 pid file 或用对应命令停止")
        }
        _ => state.kernel.stop().await,
    }
}

pub async fn restart(state: &AppState) -> Result<OperationStatus> {
    let snapshot = state.kernel.external_snapshot().await;
    let status = match snapshot.owner {
        KernelOwner::Systemd if is_active(snapshot.state) => delegate_systemd(state, "restart", &snapshot).await?,
        KernelOwner::Supervised if is_active(snapshot.state) => {
            bail!("Core 由外部 supervisor 管理，请使用对应服务命令重启")
        }
        KernelOwner::External if is_active(snapshot.state) => {
            bail!("Core 为外部接管进程，默认不重启；请确认来源后清理 pid file 或用对应命令重启")
        }
        _ => state.kernel.restart().await?,
    };
    apply_saved_proxy_selections_if_core_active(state, &status).await;
    Ok(status)
}

pub async fn run(state: &AppState) -> Result<()> {
    let (owner, detail) = foreground_owner_from_env();
    state.kernel.run_foreground(owner, detail).await
}

fn foreground_owner_from_env() -> (KernelOwner, Option<String>) {
    let owner = match std::env::var(ENV_CORE_OWNER).ok().as_deref() {
        Some("systemd") => KernelOwner::Systemd,
        Some("supervised") => KernelOwner::Supervised,
        Some("detached") => KernelOwner::Detached,
        _ => KernelOwner::Supervised,
    };
    let detail = std::env::var(ENV_SERVICE_NAME)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| matches!(owner, KernelOwner::Systemd).then(|| DEFAULT_SYSTEMD_SERVICE_NAME.to_owned()));
    (owner, detail)
}

async fn delegate_systemd(state: &AppState, action: &str, snapshot: &KernelSnapshot) -> Result<OperationStatus> {
    let service = systemd_service_name(snapshot);
    delegate_systemd_service(state, action, &service).await
}

async fn delegate_systemd_service(state: &AppState, action: &str, service: &str) -> Result<OperationStatus> {
    run_systemctl(action, service).await?;
    sleep(timeouts::SYSTEMD_SETTLE_DELAY).await;
    let snapshot = state.kernel.external_snapshot().await;
    Ok(OperationStatus {
        accepted: true,
        current_job: None,
        state: snapshot.state,
        owner: snapshot.owner,
        message: Some(format!("已委托 systemd 执行 {action} {service}")),
    })
}

async fn start_systemd_service_if_available(state: &AppState) -> Result<Option<OperationStatus>> {
    let service = configured_systemd_service_name();
    if !systemd_service_loaded(&service).await {
        return Ok(None);
    }

    Ok(Some(delegate_systemd_service(state, "start", &service).await?))
}

async fn systemd_service_loaded(service: &str) -> bool {
    let Ok(output) = Command::new("systemctl")
        .arg("show")
        .arg(service)
        .arg("-p")
        .arg("LoadState")
        .arg("--value")
        .output()
        .await
    else {
        return false;
    };
    output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "loaded"
}

async fn run_systemctl(action: &str, service: &str) -> Result<()> {
    let output = Command::new("systemctl")
        .arg(action)
        .arg(service)
        .output()
        .await
        .with_context(
            || "无法执行 systemctl；当前系统可能没有 systemd，请使用对应 supervisor 命令或手动 core start/stop",
        )?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let message = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    bail!("systemctl {action} {service} failed: {message}");
}

fn systemd_service_name(snapshot: &KernelSnapshot) -> String {
    snapshot
        .owner_detail
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(configured_systemd_service_name)
}

fn configured_systemd_service_name() -> String {
    std::env::var(ENV_SERVICE_NAME)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(installed_layout_service_name)
        .unwrap_or_else(|| DEFAULT_SYSTEMD_SERVICE_NAME.to_owned())
}

fn installed_layout_service_name() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let exe = fs::canonicalize(&exe).unwrap_or(exe);
    let content = fs::read_to_string(exe.parent()?.join("install-layout.env")).ok()?;
    content.lines().find_map(|line| {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return None;
        }
        let (key, value) = trimmed.split_once('=')?;
        (key.trim() == ENV_SERVICE_NAME)
            .then(|| value.trim())
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

const fn is_active(state: KernelState) -> bool {
    matches!(
        state,
        KernelState::Running | KernelState::Starting | KernelState::Restarting | KernelState::Unhealthy
    )
}

fn owner_message_suffix(owner: KernelOwner, detail: Option<&str>) -> String {
    let label = match owner {
        KernelOwner::Stopped => "未运行",
        KernelOwner::Detached => "手动进程",
        KernelOwner::Supervised => "外部 supervisor",
        KernelOwner::Systemd => "systemd",
        KernelOwner::External => "外部进程",
    };
    detail
        .filter(|detail| !detail.trim().is_empty())
        .map_or_else(|| format!(" {label} "), |detail| format!(" {label}（{detail}）"))
}

async fn apply_saved_proxy_selections_if_core_active(state: &AppState, status: &OperationStatus) {
    if matches!(
        status.state,
        clash_core::KernelState::Running
            | clash_core::KernelState::Starting
            | clash_core::KernelState::Restarting
            | clash_core::KernelState::Unhealthy
    ) {
        let _ =
            controller::apply_saved_proxy_selections_with_retry(state, timeouts::SAVED_PROXY_SELECTION_APPLY_TIMEOUT)
                .await;
    }
}
