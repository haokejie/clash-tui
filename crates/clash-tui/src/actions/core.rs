use std::time::Duration;

use anyhow::Result;
use clash_core::{KernelSnapshot, OperationStatus};

use crate::{actions::controller, state::AppState};

const APPLY_SAVED_SELECTION_TIMEOUT: Duration = Duration::from_secs(8);

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
    let status = state.kernel.start().await?;
    apply_saved_proxy_selections_if_core_active(state, &status).await;
    Ok(status)
}

pub async fn stop(state: &AppState) -> Result<OperationStatus> {
    state.kernel.stop().await
}

pub async fn restart(state: &AppState) -> Result<OperationStatus> {
    let status = state.kernel.restart().await?;
    apply_saved_proxy_selections_if_core_active(state, &status).await;
    Ok(status)
}

async fn apply_saved_proxy_selections_if_core_active(state: &AppState, status: &OperationStatus) {
    if matches!(
        status.state,
        clash_core::KernelState::Running
            | clash_core::KernelState::Starting
            | clash_core::KernelState::Restarting
            | clash_core::KernelState::Unhealthy
    ) {
        let _ = controller::apply_saved_proxy_selections_with_retry(state, APPLY_SAVED_SELECTION_TIMEOUT).await;
    }
}
