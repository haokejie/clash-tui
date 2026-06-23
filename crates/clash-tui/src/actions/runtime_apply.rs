use std::{path::Path, time::Duration};

use anyhow::{Result, bail};
use clash_core::{KernelState, RuntimeConfigResult, ValidationOutcome};
use serde::{Deserialize, Serialize};
use tokio::time::{Instant, sleep};

use crate::{mihomo_controller::MihomoController, state::AppState};

use super::controller::ProxySelectionApplyReport;

const APPLY_SAVED_SELECTION_TIMEOUT: Duration = Duration::from_secs(8);
const RELOAD_CONTROLLER_READY_TIMEOUT: Duration = Duration::from_secs(8);
const RELOAD_CONTROLLER_READY_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy)]
pub struct RuntimeApplyOptions {
    pub controller_ready_timeout: Duration,
}

impl Default for RuntimeApplyOptions {
    fn default() -> Self {
        Self {
            controller_ready_timeout: RELOAD_CONTROLLER_READY_TIMEOUT,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeApplyResult {
    pub runtime_path: String,
    pub runtime_validated: bool,
    pub reload_attempted: bool,
    pub runtime_reloaded: bool,
    pub core_state: KernelState,
    pub selection_report: Option<ProxySelectionApplyReport>,
}

pub async fn generate_validate_and_apply(state: &AppState) -> Result<RuntimeApplyResult> {
    generate_validate_and_apply_with_options(state, RuntimeApplyOptions::default()).await
}

pub async fn generate_validate_and_apply_with_options(
    state: &AppState,
    options: RuntimeApplyOptions,
) -> Result<RuntimeApplyResult> {
    let runtime = state.runtime.generate().await?;
    validate_and_apply_generated_with_options(state, runtime, options).await
}

pub async fn validate_and_apply_generated_with_options(
    state: &AppState,
    runtime: RuntimeConfigResult,
    options: RuntimeApplyOptions,
) -> Result<RuntimeApplyResult> {
    validate_runtime(state, &runtime).await?;

    let snapshot = state.kernel.external_snapshot().await;
    let reload_attempted = should_reload_runtime(snapshot.state);
    let selection_report = if reload_attempted {
        wait_for_controller_ready(state, options.controller_ready_timeout).await?;
        super::controller::reload_config(state, &runtime.path, true).await?;
        Some(super::controller::apply_saved_proxy_selections_with_retry(state, APPLY_SAVED_SELECTION_TIMEOUT).await)
    } else {
        None
    };

    Ok(RuntimeApplyResult {
        runtime_path: runtime.path,
        runtime_validated: true,
        reload_attempted,
        runtime_reloaded: reload_attempted,
        core_state: snapshot.state,
        selection_report,
    })
}

async fn wait_for_controller_ready(state: &AppState, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;

    loop {
        let health = MihomoController::new(state.mihomo.clone()).health().await;
        if health.healthy {
            return Ok(());
        }

        if Instant::now() >= deadline {
            bail!(
                "mihomo controller not ready for runtime reload after {}s: {}",
                timeout.as_secs(),
                health.message.as_deref().unwrap_or("unknown")
            );
        }
        sleep(RELOAD_CONTROLLER_READY_INTERVAL).await;
    }
}

async fn validate_runtime(state: &AppState, runtime: &RuntimeConfigResult) -> Result<()> {
    let outcome = crate::validation::validate_runtime_file(state, Path::new(&runtime.path)).await;
    if outcome.is_valid() {
        return Ok(());
    }

    bail!("runtime config validation failed: {}", validation_message(&outcome));
}

fn validation_message(outcome: &ValidationOutcome) -> String {
    match outcome {
        ValidationOutcome::Invalid { message, .. } => message.clone(),
        ValidationOutcome::Skipped { reason } => format!("validation skipped: {reason}"),
        ValidationOutcome::Busy => "validation is already running".into(),
        ValidationOutcome::Valid => "configuration is valid".into(),
    }
}

const fn should_reload_runtime(state: KernelState) -> bool {
    matches!(
        state,
        KernelState::Running | KernelState::Unhealthy | KernelState::Starting | KernelState::Restarting
    )
}

#[cfg(test)]
mod tests {
    use clash_core::KernelState;

    use super::should_reload_runtime;

    #[test]
    fn runtime_reload_only_targets_live_or_transient_core_states() {
        assert!(!should_reload_runtime(KernelState::Stopped));
        assert!(!should_reload_runtime(KernelState::Crashed));
        assert!(!should_reload_runtime(KernelState::Stopping));
        assert!(!should_reload_runtime(KernelState::Updating));
        assert!(should_reload_runtime(KernelState::Running));
        assert!(should_reload_runtime(KernelState::Unhealthy));
        assert!(should_reload_runtime(KernelState::Starting));
        assert!(should_reload_runtime(KernelState::Restarting));
    }
}
