use std::{path::Path, process::Stdio, sync::Arc, time::Duration};

use anyhow::{Context as _, Result, bail};
use clash_core::{
    KernelState, OperationStatus, RuntimeConfigResult, ValidationErrorKind, ValidationOutcome, config::dns, yaml,
};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::{process::Command, time::timeout};

use crate::{actions::core, jobs::JobRecord, state::AppState};

const RUNTIME_VALIDATE_JOB_KIND: &str = "runtime-config-validate";
const DNS_VALIDATE_JOB_KIND: &str = "dns-config-validate";
const VALIDATION_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct StartedValidationJob {
    pub job: JobRecord,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigValidationResult {
    pub outcome: ValidationOutcome,
    pub runtime: Option<RuntimeConfigResult>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsApplyResult {
    pub applied: bool,
    pub runtime: RuntimeConfigResult,
    pub restart: Option<OperationStatus>,
}

pub async fn dns_config_exists(state: &AppState) -> bool {
    tokio::fs::try_exists(&state.store.paths().dns_config)
        .await
        .unwrap_or(false)
}

pub async fn get_dns_config_content(state: &AppState) -> Result<String> {
    let path = &state.store.paths().dns_config;
    if !dns_config_exists(state).await {
        bail!("DNS config file not found");
    }
    tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))
}

pub async fn save_dns_config(state: &AppState, dns_config: Value) -> Result<Value> {
    if !dns_config.is_object() {
        bail!("DNS config root must be an object");
    }

    let yaml_text = serde_yaml_ng::to_string(&dns_config).context("failed to serialize DNS config")?;
    let mut yaml_value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&yaml_text).context("failed to parse DNS config")?;
    yaml_value
        .apply_merge()
        .context("failed to apply DNS config yaml merge")?;
    let mapping = yaml_value
        .as_mapping()
        .cloned()
        .context("DNS config root must be a mapping")?;

    state.store.paths().ensure_dirs()?;
    yaml::save_yaml(&state.store.paths().dns_config, &mapping, Some(dns::DNS_CONFIG_HEADER)).await?;
    Ok(dns_config)
}

pub async fn apply_dns_config(state: Arc<AppState>, apply: bool) -> Result<DnsApplyResult> {
    if apply {
        dns::ensure_dns_config(&state.store.paths().dns_config).await?;
    }

    let runtime = state.runtime.generate_with_dns_override(Some(apply)).await?;
    let snapshot = state.kernel.external_snapshot().await;
    let restart = if matches!(snapshot.state, KernelState::Running | KernelState::Unhealthy) {
        Some(core::restart(state.as_ref()).await?)
    } else {
        None
    };

    Ok(DnsApplyResult {
        applied: apply,
        runtime,
        restart,
    })
}

pub async fn start_runtime_validation_job(state: Arc<AppState>) -> StartedValidationJob {
    let (job, created) = state
        .jobs
        .create_unique_active(RUNTIME_VALIDATE_JOB_KIND, "Validate runtime config", None)
        .await;

    if created {
        let job_id = job.id.clone();
        let task_job_id = job_id.clone();
        let jobs = state.jobs.clone();
        let handle = tokio::spawn(async move {
            run_runtime_validation_job(state, task_job_id).await;
        });
        jobs.register_abort_handle(&job_id, handle.abort_handle()).await;
    }

    StartedValidationJob { job }
}

pub async fn start_dns_validation_job(state: Arc<AppState>) -> StartedValidationJob {
    let (job, created) = state
        .jobs
        .create_unique_active(DNS_VALIDATE_JOB_KIND, "Validate DNS config", Some("dns".into()))
        .await;

    if created {
        let job_id = job.id.clone();
        let task_job_id = job_id.clone();
        let jobs = state.jobs.clone();
        let handle = tokio::spawn(async move {
            run_dns_validation_job(state, task_job_id).await;
        });
        jobs.register_abort_handle(&job_id, handle.abort_handle()).await;
    }

    StartedValidationJob { job }
}

async fn run_runtime_validation_job(state: Arc<AppState>, job_id: String) {
    state.jobs.start(&job_id, "validating runtime config").await;
    let result = validate_runtime_config(&state, None).await;
    finish_validation_job(&state, &job_id, "runtime config", result).await;
}

async fn run_dns_validation_job(state: Arc<AppState>, job_id: String) {
    state.jobs.start(&job_id, "validating DNS config").await;
    let result = validate_dns_config(&state).await;
    finish_validation_job(&state, &job_id, "DNS config", result).await;
}

async fn finish_validation_job(state: &AppState, job_id: &str, label: &str, result: ConfigValidationResult) {
    let message = if result.outcome.is_valid() {
        format!("{label} is valid")
    } else {
        format!("{label} is invalid")
    };
    let result = serde_json::to_value(result).unwrap_or_else(|err| {
        json!({
            "outcome": ValidationOutcome::invalid_from_message(format!("failed to serialize validation result: {err}")),
            "runtime": null,
        })
    });
    state.jobs.finish(job_id, message, Some(result)).await;
}

async fn validate_dns_config(state: &AppState) -> ConfigValidationResult {
    if !dns_config_exists(state).await {
        return ConfigValidationResult {
            outcome: ValidationOutcome::invalid(ValidationErrorKind::FileMissing, "DNS config file not found"),
            runtime: None,
        };
    }

    validate_runtime_config(state, Some(true)).await
}

async fn validate_runtime_config(state: &AppState, enable_dns_settings: Option<bool>) -> ConfigValidationResult {
    let runtime = match state.runtime.generate_with_dns_override(enable_dns_settings).await {
        Ok(runtime) => runtime,
        Err(err) => {
            return ConfigValidationResult {
                outcome: ValidationOutcome::invalid_from_message(format!("failed to generate runtime config: {err}")),
                runtime: None,
            };
        }
    };

    let outcome = validate_runtime_file(state, Path::new(&runtime.path)).await;
    ConfigValidationResult {
        outcome,
        runtime: Some(runtime),
    }
}

pub async fn validate_runtime_file(state: &AppState, config_path: &Path) -> ValidationOutcome {
    if !tokio::fs::try_exists(config_path).await.unwrap_or(false) {
        return ValidationOutcome::invalid(
            ValidationErrorKind::FileMissing,
            format!("File not found: {}", config_path.display()),
        );
    }

    let mihomo_bin = state.options.resolved_mihomo_bin(state.store.paths());
    let mut command = Command::new(&mihomo_bin);
    command
        .arg("-t")
        .arg("-d")
        .arg(&state.store.paths().home_dir)
        .arg("-f")
        .arg(config_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let output = match timeout(VALIDATION_TIMEOUT, command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => {
            return ValidationOutcome::invalid_from_message(format!(
                "failed to run mihomo validation {}: {err}",
                mihomo_bin.display()
            ));
        }
        Err(_) => {
            return ValidationOutcome::invalid(
                ValidationErrorKind::Timeout,
                format!("validation timeout after {}s", VALIDATION_TIMEOUT.as_secs()),
            );
        }
    };

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let error_keywords = ["FATA", "fatal", "Parse config error", "level=fatal"];
    let has_error = !output.status.success()
        || error_keywords
            .iter()
            .any(|keyword| stderr.contains(keyword) || stdout.contains(keyword));

    if has_error {
        let message = if !stdout.trim().is_empty() {
            stdout.trim().to_owned()
        } else if !stderr.trim().is_empty() {
            stderr.trim().to_owned()
        } else if let Some(code) = output.status.code() {
            format!("validation process exited with code {code}")
        } else {
            "validation process was terminated".into()
        };

        if output.status.code().is_none() {
            ValidationOutcome::invalid(ValidationErrorKind::ProcessTerminated, message)
        } else {
            ValidationOutcome::invalid_from_message(message)
        }
    } else {
        ValidationOutcome::Valid
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::{
        fs,
        sync::Arc,
        time::{SystemTime, UNIX_EPOCH},
    };

    use serde_json::json;
    use serde_yaml_ng::Value;

    use crate::{options::ClashTuiOptions, state::AppState};

    #[tokio::test]
    async fn save_dns_config_rejects_non_object_root() {
        let root = temp_root("dns-config");
        let _ = fs::remove_dir_all(&root);
        let state = AppState::initialize(
            ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
        )
        .await
        .expect("state");

        let error = super::save_dns_config(&state, json!(["not", "an", "object"]))
            .await
            .expect_err("invalid DNS config");

        assert!(error.to_string().contains("DNS config root must be an object"));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn apply_dns_config_creates_default_config_when_missing() {
        let root = temp_root("dns-config-missing");
        let _ = fs::remove_dir_all(&root);
        let state = AppState::initialize(
            ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
        )
        .await
        .expect("state");
        let dns_path = state.store.paths().dns_config.clone();
        fs::remove_file(&dns_path).expect("remove dns config");

        let result = super::apply_dns_config(Arc::clone(&state), true)
            .await
            .expect("apply dns");

        assert!(result.applied);
        assert!(dns_path.is_file());
        let runtime = clash_core::yaml::read_mapping(&state.store.paths().runtime_config)
            .await
            .expect("read runtime");
        assert!(runtime.get("dns").and_then(Value::as_mapping).is_some());
        let _ = fs::remove_dir_all(root);
    }

    fn temp_root(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        std::env::temp_dir().join(format!("clash-tui-{name}-{}-{nanos}", std::process::id()))
    }
}
