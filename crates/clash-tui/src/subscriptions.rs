use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use clash_core::{IProfiles, PrfItem, PrfOption};
use serde::Serialize;

use crate::{
    jobs::{JobRecord, JobStatus},
    state::AppState,
};

const PROFILE_UPDATE_JOB_KIND: &str = "profile-update";
#[derive(Debug, Clone, Serialize)]
pub struct StartedProfileUpdateJob {
    pub job: JobRecord,
    pub created: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionSweep {
    pub checked: usize,
    pub due: usize,
    pub queued: usize,
    pub skipped: usize,
    pub jobs: Vec<JobRecord>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionStatus {
    pub scheduler: SubscriptionSchedulerStatus,
    pub profiles: Vec<SubscriptionProfileStatus>,
    pub jobs: Vec<JobRecord>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionSchedulerStatus {
    /// Background interval scheduler state. Clash TUI only performs a one-shot
    /// startup sweep; this remains for JSON compatibility.
    pub enabled: bool,
    /// Compatibility field for older clients. No background timer uses this value.
    pub check_interval_secs: u64,
    pub next_check_in_secs: Option<u64>,
    pub startup_check_enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionProfileStatus {
    pub uid: Option<String>,
    pub name: Option<String>,
    pub remote: bool,
    pub auto_update_enabled: bool,
    pub update_interval_minutes: Option<u64>,
    pub updated_at: Option<u64>,
    pub next_update_at: Option<u64>,
    pub due: bool,
    pub due_reason: String,
    pub active_job: Option<JobRecord>,
    pub latest_job: Option<JobRecord>,
    pub latest_result: Option<String>,
    pub latest_failure: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ProfileUpdateRuntimeRefresh {
    current_profile: bool,
    runtime_path: Option<String>,
    runtime_validated: bool,
    runtime_reloaded: bool,
    warning: Option<String>,
}

pub fn spawn_subscription_startup_sweep(state: Arc<AppState>) {
    tokio::spawn(async move {
        if let Err(err) = enqueue_due_profile_updates(Arc::clone(&state)).await {
            eprintln!("subscription startup sweep failed: {err}");
        }
    });
}

pub async fn status(state: &AppState) -> Result<SubscriptionStatus> {
    let profiles = state.store.load_profiles().await?;
    state.config.write().await.profiles = profiles.clone();
    let jobs = state.jobs.list().await;
    let now = current_timestamp_secs();
    let profile_statuses = profiles
        .items
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|item| subscription_profile_status(item, &jobs, now))
        .collect();

    Ok(SubscriptionStatus {
        scheduler: subscription_scheduler_status(state.options.subscription_check_interval_secs),
        profiles: profile_statuses,
        jobs,
    })
}

const fn subscription_scheduler_status(check_interval_secs: u64) -> SubscriptionSchedulerStatus {
    SubscriptionSchedulerStatus {
        enabled: false,
        check_interval_secs,
        next_check_in_secs: None,
        startup_check_enabled: true,
    }
}

pub async fn enqueue_due_profile_updates(state: Arc<AppState>) -> Result<SubscriptionSweep> {
    let profiles = state.store.load_profiles().await?;
    state.config.write().await.profiles = profiles.clone();

    let now = current_timestamp_secs();
    let checked = profiles.items.as_deref().map_or(0, <[PrfItem]>::len);
    let due_uids = due_profile_uids(&profiles, now);
    let due = due_uids.len();
    let mut jobs = Vec::new();
    let mut queued = 0;

    for uid in due_uids {
        let started = start_profile_update_job(Arc::clone(&state), uid, None).await;
        if started.created {
            queued += 1;
        }
        jobs.push(started.job);
    }

    Ok(SubscriptionSweep {
        checked,
        due,
        queued,
        skipped: due.saturating_sub(queued),
        jobs,
    })
}

pub async fn enqueue_all_profile_updates(state: Arc<AppState>) -> Result<SubscriptionSweep> {
    let profiles = state.store.load_profiles().await?;
    state.config.write().await.profiles = profiles.clone();

    let remote_uids = remote_profile_uids(&profiles);
    let checked = profiles.items.as_deref().map_or(0, <[PrfItem]>::len);
    let due = remote_uids.len();
    let mut jobs = Vec::new();
    let mut queued = 0;

    for uid in remote_uids {
        let started = start_profile_update_job(Arc::clone(&state), uid, None).await;
        if started.created {
            queued += 1;
        }
        jobs.push(started.job);
    }

    Ok(SubscriptionSweep {
        checked,
        due,
        queued,
        skipped: due.saturating_sub(queued),
        jobs,
    })
}

pub async fn start_profile_update_job(
    state: Arc<AppState>,
    uid: String,
    option: Option<PrfOption>,
) -> StartedProfileUpdateJob {
    let (job, created) = state
        .jobs
        .create_unique_active(
            PROFILE_UPDATE_JOB_KIND,
            format!("Update remote profile {uid}"),
            Some(uid.clone()),
        )
        .await;

    if created {
        let job_id = job.id.clone();
        let task_job_id = job_id.clone();
        let jobs = state.jobs.clone();
        let handle = tokio::spawn(async move {
            run_profile_update_job(state, task_job_id, uid, option).await;
        });
        jobs.register_abort_handle(&job_id, handle.abort_handle()).await;
    }

    StartedProfileUpdateJob { job, created }
}

async fn run_profile_update_job(state: Arc<AppState>, job_id: String, uid: String, option: Option<PrfOption>) {
    state
        .jobs
        .start(&job_id, "downloading remote profile via direct connection")
        .await;

    let rollback = match capture_update_backup_if_current(&state, &uid).await {
        Ok(rollback) => rollback,
        Err(err) => {
            state.jobs.fail(&job_id, err.to_string()).await;
            return;
        }
    };

    match update_remote_profile_with_retry(&state, &job_id, &uid, option.as_ref()).await {
        Ok(profiles) => {
            state.jobs.progress(&job_id, "saving profile metadata").await;
            state.config.write().await.profiles = profiles.clone();
            match refresh_current_runtime_after_profile_update(&state, &uid, &profiles).await {
                Ok(runtime_refresh) => {
                    let result = Some(profile_update_job_result(&uid, &profiles, &runtime_refresh));
                    let message = profile_update_success_message(&runtime_refresh);
                    state.jobs.finish(&job_id, message, result).await;
                }
                Err(err) => {
                    let rollback_message = match rollback.as_ref() {
                        Some(rollback) => match rollback.restore(&state).await {
                            Ok(()) => "；已回滚订阅文件".to_owned(),
                            Err(rollback_err) => format!("；回滚订阅文件失败：{rollback_err}"),
                        },
                        None => String::new(),
                    };
                    state
                        .jobs
                        .fail(
                            &job_id,
                            format!("订阅已下载，但运行配置应用失败：{err}{rollback_message}"),
                        )
                        .await;
                }
            }
        }
        Err(err) => {
            state.jobs.fail(&job_id, err.to_string()).await;
        }
    }
}

async fn refresh_current_runtime_after_profile_update(
    state: &AppState,
    uid: &str,
    profiles: &IProfiles,
) -> Result<ProfileUpdateRuntimeRefresh> {
    if profiles.current.as_deref() != Some(uid) {
        return Ok(ProfileUpdateRuntimeRefresh::default());
    }

    let _guard = state.profile_switch_lock.lock().await;
    let latest_profiles = state.store.load_profiles().await?;
    if latest_profiles.current.as_deref() != Some(uid) {
        state.config.write().await.profiles = latest_profiles;
        return Ok(ProfileUpdateRuntimeRefresh::default());
    }
    state.config.write().await.profiles = latest_profiles;

    let apply = crate::actions::runtime_apply::generate_validate_and_apply(state).await?;
    Ok(ProfileUpdateRuntimeRefresh {
        current_profile: true,
        runtime_path: Some(apply.runtime_path),
        runtime_validated: apply.runtime_validated,
        runtime_reloaded: apply.runtime_reloaded,
        warning: None,
    })
}

const fn profile_update_success_message(runtime_refresh: &ProfileUpdateRuntimeRefresh) -> &'static str {
    if !runtime_refresh.current_profile {
        "订阅已更新"
    } else if runtime_refresh.warning.is_some() {
        "订阅已更新；运行配置刷新需要处理"
    } else if runtime_refresh.runtime_reloaded {
        "订阅已更新；运行配置已热加载"
    } else {
        "订阅已更新；运行配置已刷新，Core 启动后应用"
    }
}

async fn update_remote_profile_with_retry(
    state: &AppState,
    job_id: &str,
    uid: &str,
    option: Option<&PrfOption>,
) -> Result<IProfiles> {
    let attempts = [
        ("direct", Some(proxy_option(option, Some(false), Some(false)))),
        ("Clash proxy", Some(proxy_option(option, Some(false), Some(true)))),
        ("system proxy", Some(proxy_option(option, Some(true), Some(false)))),
    ];
    let mut errors = Vec::new();

    for (label, attempt_option) in attempts {
        state
            .jobs
            .progress(job_id, format!("downloading remote profile via {label}"))
            .await;
        match state.store.update_remote_profile(uid, attempt_option.as_ref()).await {
            Ok(profiles) => return Ok(profiles),
            Err(err) => errors.push(format!("{label}: {}", redact_urls(&err.to_string()))),
        }
    }

    anyhow::bail!("all subscription update attempts failed: {}", errors.join("; "))
}

fn proxy_option(base: Option<&PrfOption>, with_proxy: Option<bool>, self_proxy: Option<bool>) -> PrfOption {
    let mut option = base.cloned().unwrap_or_default();
    option.with_proxy = with_proxy;
    option.self_proxy = self_proxy;
    option
}

fn redact_urls(message: &str) -> String {
    message
        .split_whitespace()
        .map(|part| {
            if part.starts_with("http://") || part.starts_with("https://") {
                "[redacted-url]"
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

async fn capture_update_backup_if_current(state: &AppState, uid: &str) -> Result<Option<ProfileUpdateBackup>> {
    let profiles = state.store.load_profiles().await?;
    if profiles.current.as_deref() != Some(uid) {
        return Ok(None);
    }
    ProfileUpdateBackup::capture(state, &profiles, uid).await.map(Some)
}

#[derive(Debug, Clone)]
struct ProfileUpdateBackup {
    profiles_config_path: PathBuf,
    profiles_config: Option<Vec<u8>>,
    profile_file_path: Option<PathBuf>,
    profile_file: Option<Vec<u8>>,
}

impl ProfileUpdateBackup {
    async fn capture(state: &AppState, profiles: &IProfiles, uid: &str) -> Result<Self> {
        let item = profiles.get_item(uid)?;
        let profile_file_path = item
            .file
            .as_deref()
            .map(|file| profile_file_path(state, file))
            .transpose()?;
        let profile_file = match profile_file_path.as_ref() {
            Some(path) => read_optional_file(path).await?,
            None => None,
        };
        let profiles_config_path = state.store.paths().profiles_config.clone();
        Ok(Self {
            profiles_config: read_optional_file(&profiles_config_path).await?,
            profiles_config_path,
            profile_file_path,
            profile_file,
        })
    }

    async fn restore(&self, state: &AppState) -> Result<()> {
        restore_optional_file(&self.profiles_config_path, self.profiles_config.as_deref()).await?;
        if let Some(path) = self.profile_file_path.as_ref() {
            restore_optional_file(path, self.profile_file.as_deref()).await?;
        }
        let profiles = state.store.load_profiles().await?;
        state.config.write().await.profiles = profiles;
        state.runtime.generate().await?;
        Ok(())
    }
}

fn profile_file_path(state: &AppState, file: &str) -> Result<PathBuf> {
    let relative = Path::new(file);
    if file.trim().is_empty() || relative.is_absolute() {
        anyhow::bail!("invalid profile file path");
    }

    let mut components = relative.components();
    let Some(Component::Normal(name)) = components.next() else {
        anyhow::bail!("invalid profile file path");
    };
    if components.next().is_some() {
        anyhow::bail!("profile file path must not contain directories");
    }

    Ok(state.store.paths().profiles_dir.join(name))
}

async fn read_optional_file(path: &Path) -> Result<Option<Vec<u8>>> {
    match tokio::fs::read(path).await {
        Ok(content) => Ok(Some(content)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

async fn restore_optional_file(path: &Path, content: Option<&[u8]>) -> Result<()> {
    match content {
        Some(content) => {
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(path, content).await?;
        }
        None => match tokio::fs::remove_file(path).await {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        },
    }
    Ok(())
}

fn due_profile_uids(profiles: &IProfiles, now: u64) -> Vec<String> {
    profiles
        .items
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| due_profile_uid(item, now))
        .collect()
}

fn remote_profile_uids(profiles: &IProfiles) -> Vec<String> {
    profiles
        .items
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|item| item.itype.as_deref() == Some("remote") && item.url.is_some())
        .filter_map(|item| item.uid.clone())
        .collect()
}

fn profile_update_job_result(
    uid: &str,
    profiles: &IProfiles,
    runtime_refresh: &ProfileUpdateRuntimeRefresh,
) -> serde_json::Value {
    let profile_count = profiles.items.as_deref().map_or(0, <[PrfItem]>::len);
    serde_json::json!({
        "uid": uid,
        "profileCount": profile_count,
        "current": profiles.current.as_deref(),
        "currentProfile": runtime_refresh.current_profile,
        "runtimePath": runtime_refresh.runtime_path.as_deref(),
        "runtimeValidated": runtime_refresh.runtime_validated,
        "runtimeReloaded": runtime_refresh.runtime_reloaded,
        "runtimeWarning": runtime_refresh.warning.as_deref(),
    })
}

fn due_profile_uid(item: &PrfItem, now: u64) -> Option<String> {
    if item.itype.as_deref() != Some("remote") || item.url.is_none() {
        return None;
    }

    let option = item.option.as_ref()?;
    if !option.allow_auto_update.unwrap_or(true) {
        return None;
    }

    let interval_minutes = option.update_interval?;
    if interval_minutes == 0 {
        return None;
    }

    let updated = u64::try_from(item.updated?).ok()?;
    let due_after = interval_minutes.saturating_mul(60);
    (now.saturating_sub(updated) >= due_after)
        .then(|| item.uid.clone())
        .flatten()
}

fn subscription_profile_status(item: &PrfItem, jobs: &[JobRecord], now: u64) -> SubscriptionProfileStatus {
    let uid = item.uid.clone();
    let remote = item.itype.as_deref() == Some("remote") && item.url.is_some();
    let option = item.option.as_ref();
    let auto_update_enabled = option.and_then(|option| option.allow_auto_update).unwrap_or(true);
    let update_interval_minutes = option.and_then(|option| option.update_interval);
    let updated_at = item.updated.and_then(|updated| u64::try_from(updated).ok());
    let next_update_at = updated_at
        .zip(update_interval_minutes)
        .and_then(|(updated, interval)| (interval > 0).then(|| updated.saturating_add(interval.saturating_mul(60))));
    let due = next_update_at.is_some_and(|next| now >= next) && remote && auto_update_enabled;
    let due_reason = due_reason(remote, auto_update_enabled, update_interval_minutes, updated_at, due);
    let matching_jobs = uid.as_deref().map(|uid| jobs_for_target(jobs, uid)).unwrap_or_default();
    let active_job = matching_jobs
        .iter()
        .find(|job| matches!(job.status, JobStatus::Pending | JobStatus::Running))
        .cloned();
    let latest_job = matching_jobs.last().cloned();
    let latest_result = latest_job
        .as_ref()
        .filter(|job| matches!(job.status, JobStatus::Succeeded))
        .and_then(|job| job.message.clone());
    let latest_failure = latest_job
        .as_ref()
        .filter(|job| matches!(job.status, JobStatus::Failed))
        .and_then(|job| job.error.clone().or_else(|| job.message.clone()));

    SubscriptionProfileStatus {
        uid,
        name: item.name.clone(),
        remote,
        auto_update_enabled,
        update_interval_minutes,
        updated_at,
        next_update_at,
        due,
        due_reason,
        active_job,
        latest_job,
        latest_result,
        latest_failure,
    }
}

fn jobs_for_target(jobs: &[JobRecord], target: &str) -> Vec<JobRecord> {
    jobs.iter()
        .filter(|job| job.kind == PROFILE_UPDATE_JOB_KIND && job.target.as_deref() == Some(target))
        .cloned()
        .collect::<Vec<_>>()
}

fn due_reason(
    remote: bool,
    auto_update_enabled: bool,
    update_interval_minutes: Option<u64>,
    updated_at: Option<u64>,
    due: bool,
) -> String {
    if !remote {
        return "not a remote subscription".into();
    }
    if !auto_update_enabled {
        return "auto update disabled".into();
    }
    match (update_interval_minutes, updated_at, due) {
        (None, _, _) => "missing update interval".into(),
        (Some(0), _, _) => "update interval is zero".into(),
        (_, None, _) => "missing last update timestamp".into(),
        (_, _, true) => "due now".into(),
        _ => "scheduled".into(),
    }
}

fn current_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::Path,
        time::{SystemTime, UNIX_EPOCH},
    };

    use clash_core::{IProfiles, LocalProfileImport, PrfItem, PrfOption};

    use super::{
        ProfileUpdateBackup, ProfileUpdateRuntimeRefresh, due_profile_uids, profile_update_job_result,
        profile_update_success_message, redact_urls, refresh_current_runtime_after_profile_update,
        subscription_profile_status, subscription_scheduler_status,
    };
    use crate::{options::ClashTuiOptions, state::AppState};

    fn remote(uid: &str, updated: Option<usize>, interval: Option<u64>, allow: Option<bool>) -> PrfItem {
        PrfItem {
            uid: Some(uid.into()),
            itype: Some("remote".into()),
            url: Some("https://example.test/sub".into()),
            updated,
            option: Some(PrfOption {
                update_interval: interval,
                allow_auto_update: allow,
                ..PrfOption::default()
            }),
            ..PrfItem::default()
        }
    }

    #[test]
    fn due_profile_uids_respect_interval_and_auto_update_flag() {
        let profiles = IProfiles {
            current: None,
            items: Some(vec![
                remote("due", Some(100), Some(1), Some(true)),
                remote("fresh", Some(580), Some(1), Some(true)),
                remote("disabled", Some(100), Some(1), Some(false)),
                remote("missing-updated", None, Some(1), Some(true)),
                PrfItem {
                    uid: Some("local".into()),
                    itype: Some("local".into()),
                    ..remote("local", Some(100), Some(1), Some(true))
                },
            ]),
        };

        assert_eq!(due_profile_uids(&profiles, 600), vec!["due".to_owned()]);
    }

    #[test]
    fn subscription_update_errors_redact_remote_urls() {
        let redacted = redact_urls("direct: failed https://example.test/sub?token=secret");

        assert_eq!(redacted, "direct: failed [redacted-url]");
    }

    #[test]
    fn subscription_status_reports_next_update_and_reason() {
        let status = subscription_profile_status(&remote("due", Some(100), Some(1), Some(true)), &[], 200);

        assert!(status.remote);
        assert!(status.due);
        assert_eq!(status.next_update_at, Some(160));
        assert_eq!(status.due_reason, "due now");

        let disabled = subscription_profile_status(&remote("disabled", Some(100), Some(1), Some(false)), &[], 200);

        assert!(!disabled.due);
        assert_eq!(disabled.due_reason, "auto update disabled");
    }

    #[test]
    fn subscription_scheduler_status_is_startup_only() {
        let status = subscription_scheduler_status(300);

        assert!(!status.enabled);
        assert!(status.startup_check_enabled);
        assert_eq!(status.check_interval_secs, 300);
        assert_eq!(status.next_check_in_secs, None);
    }

    #[test]
    fn profile_update_job_result_omits_remote_urls() {
        let profiles = IProfiles {
            current: Some("remote".into()),
            items: Some(vec![remote("remote", Some(100), Some(1), Some(true))]),
        };
        let runtime_refresh = ProfileUpdateRuntimeRefresh {
            current_profile: true,
            runtime_path: Some("/tmp/runtime.yaml".into()),
            runtime_validated: true,
            runtime_reloaded: false,
            warning: None,
        };

        let result = profile_update_job_result("remote", &profiles, &runtime_refresh);
        let serialized = serde_json::to_string(&result).unwrap_or_default();

        assert_eq!(result.get("uid").and_then(serde_json::Value::as_str), Some("remote"));
        assert_eq!(result.get("profileCount").and_then(serde_json::Value::as_u64), Some(1));
        assert_eq!(
            result.get("currentProfile").and_then(serde_json::Value::as_bool),
            Some(true)
        );
        assert_eq!(
            result.get("runtimePath").and_then(serde_json::Value::as_str),
            Some("/tmp/runtime.yaml")
        );
        assert!(!serialized.contains("example.test"));
        assert!(!serialized.contains("https://"));
    }

    #[test]
    fn profile_update_success_message_reports_current_profile_runtime_refresh() {
        assert_eq!(
            profile_update_success_message(&ProfileUpdateRuntimeRefresh::default()),
            "订阅已更新"
        );
        assert_eq!(
            profile_update_success_message(&ProfileUpdateRuntimeRefresh {
                current_profile: true,
                runtime_path: Some("/tmp/runtime.yaml".into()),
                runtime_validated: true,
                runtime_reloaded: false,
                warning: None,
            }),
            "订阅已更新；运行配置已刷新，Core 启动后应用"
        );
        assert_eq!(
            profile_update_success_message(&ProfileUpdateRuntimeRefresh {
                current_profile: true,
                runtime_path: Some("/tmp/runtime.yaml".into()),
                runtime_validated: true,
                runtime_reloaded: true,
                warning: None,
            }),
            "订阅已更新；运行配置已热加载"
        );
        assert_eq!(
            profile_update_success_message(&ProfileUpdateRuntimeRefresh {
                current_profile: true,
                warning: Some("runtime failed".into()),
                ..ProfileUpdateRuntimeRefresh::default()
            }),
            "订阅已更新；运行配置刷新需要处理"
        );
    }

    #[tokio::test]
    async fn current_profile_update_regenerates_runtime_without_core() {
        let root = temp_root("subscription-current-runtime");
        let _ = fs::remove_dir_all(&root);
        install_fake_mihomo(&root);
        let options =
            ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options");
        let state = AppState::initialize(options).await.expect("state");

        state
            .store
            .import_local_profile(&LocalProfileImport {
                uid: Some("Lcurrent".into()),
                name: Some("Current".into()),
                file_data: r"
proxies:
  - name: UniqueNode
    type: direct
proxy-groups:
  - name: UniqueGroup
    type: select
    proxies:
      - UniqueNode
      - DIRECT
rules: []
"
                .into(),
            })
            .await
            .expect("current profile");
        let profiles = state.store.load_profiles().await.expect("profiles");

        let refresh = refresh_current_runtime_after_profile_update(&state, "Lcurrent", &profiles)
            .await
            .expect("runtime refresh");

        assert!(refresh.current_profile);
        assert!(
            refresh
                .runtime_path
                .as_deref()
                .is_some_and(|path| std::path::Path::new(path).is_file())
        );
        assert!(refresh.runtime_validated);
        assert!(!refresh.runtime_reloaded);
        assert!(refresh.warning.is_none());
        let runtime = tokio::fs::read_to_string(refresh.runtime_path.expect("runtime path"))
            .await
            .expect("runtime");
        assert!(runtime.contains("UniqueGroup"));
        assert!(runtime.contains("UniqueNode"));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn non_current_profile_update_does_not_regenerate_runtime() {
        let root = temp_root("subscription-non-current-runtime");
        let _ = fs::remove_dir_all(&root);
        let options =
            ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options");
        let state = AppState::initialize(options).await.expect("state");

        state
            .store
            .import_local_profile(&LocalProfileImport {
                uid: Some("Lcurrent".into()),
                name: Some("Current".into()),
                file_data: "mode: global\nproxies: []\nproxy-groups: []\nrules: []\n".into(),
            })
            .await
            .expect("current profile");
        state
            .store
            .import_local_profile(&LocalProfileImport {
                uid: Some("Lother".into()),
                name: Some("Other".into()),
                file_data: "mode: rule\nproxies: []\nproxy-groups: []\nrules: []\n".into(),
            })
            .await
            .expect("other profile");
        let profiles = state.store.load_profiles().await.expect("profiles");

        let refresh = refresh_current_runtime_after_profile_update(&state, "Lother", &profiles)
            .await
            .expect("runtime refresh");

        assert_eq!(refresh, ProfileUpdateRuntimeRefresh::default());
        assert!(!state.store.paths().runtime_config.exists());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn profile_update_rollback_reports_runtime_regeneration_failure() {
        let root = temp_root("subscription-rollback-runtime-failure");
        let _ = fs::remove_dir_all(&root);
        install_fake_mihomo(&root);
        let options =
            ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options");
        let state = AppState::initialize(options).await.expect("state");
        let profiles = IProfiles {
            current: Some("Lbroken".into()),
            items: Some(vec![PrfItem {
                uid: Some("Lbroken".into()),
                itype: Some("local".into()),
                name: Some("Broken".into()),
                file: Some("broken.yaml".into()),
                ..PrfItem::default()
            }]),
        };
        profiles
            .save_file(&state.store.paths().profiles_config)
            .await
            .expect("profiles");
        let backup = ProfileUpdateBackup {
            profiles_config_path: state.store.paths().profiles_config.clone(),
            profiles_config: Some(
                tokio::fs::read(&state.store.paths().profiles_config)
                    .await
                    .expect("read"),
            ),
            profile_file_path: Some(state.store.paths().profiles_dir.join("broken.yaml")),
            profile_file: Some(b"proxy-groups: [".to_vec()),
        };

        let err = backup
            .restore(&state)
            .await
            .expect_err("runtime regeneration should fail");

        assert!(
            err.to_string().contains("broken.yaml"),
            "error should include broken profile file: {err:#}"
        );

        let _ = fs::remove_dir_all(root);
    }

    fn temp_root(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        std::env::temp_dir().join(format!("clash-tui-{name}-{}-{nanos}", std::process::id()))
    }

    fn install_fake_mihomo(root: &Path) {
        let resources = root.join("resources");
        fs::create_dir_all(&resources).expect("resources");
        let mihomo = resources.join("mihomo");
        fs::write(
            &mihomo,
            "#!/bin/sh\nif [ \"$1\" = \"-t\" ]; then exit 0; fi\nprintf 'Mihomo Meta vtest\\n'\n",
        )
        .expect("fake mihomo");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut permissions = fs::metadata(&mihomo).expect("metadata").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&mihomo, permissions).expect("chmod");
        }
    }
}
