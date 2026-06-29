use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use clash_core::{
    IProfiles, PrfItem, PrfOption,
    config::{
        profiles::{RemoteProfileDownload, download_remote_profile},
        store::RemoteProfileUpdatePlan,
    },
};
use serde::Serialize;

use crate::{
    actions::profile_transaction::{
        ProfileSnapshotSpec, ProfileTransactionLock, ProfileTransactionSpec, RuntimeCommitOptions, RuntimePolicy,
        run_profile_transaction,
    },
    jobs::{JobRecord, JobStatus},
    state::AppState,
};

const PROFILE_UPDATE_JOB_KIND: &str = "profile-update";

#[derive(Debug, Clone)]
struct PreparedProfileUpdate {
    plan: RemoteProfileUpdatePlan,
    remote: RemoteProfileDownload,
}

#[derive(Debug, Clone)]
struct ProfileUpdateCommitResult {
    profiles: IProfiles,
    runtime_refresh: ProfileUpdateRuntimeRefresh,
}
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

    match update_remote_profile_with_retry(&state, &job_id, &uid, option.as_ref()).await {
        Ok(prepared) => {
            state.jobs.progress(&job_id, "saving profile metadata").await;
            match commit_profile_update_transaction(&state, &uid, prepared).await {
                Ok(commit) => {
                    let result = Some(profile_update_job_result(
                        &uid,
                        &commit.profiles,
                        &commit.runtime_refresh,
                    ));
                    let message = profile_update_success_message(&commit.runtime_refresh);
                    state.jobs.finish(&job_id, message, result).await;
                }
                Err(err) => {
                    state
                        .jobs
                        .fail(&job_id, format!("订阅已下载，但配置提交或运行配置应用失败：{err}"))
                        .await;
                }
            }
        }
        Err(err) => {
            state.jobs.fail(&job_id, err.to_string()).await;
        }
    }
}

async fn commit_profile_update_transaction(
    state: &AppState,
    uid: &str,
    prepared: PreparedProfileUpdate,
) -> Result<ProfileUpdateCommitResult> {
    let uid = uid.to_owned();
    let outcome =
        run_profile_transaction(
            state,
            ProfileTransactionSpec {
                failure_context: "subscription profile update failed",
                rollback_success_message: "subscription profile was rolled back",
                rollback_failed_message: "subscription profile rollback failed",
                lock: ProfileTransactionLock::Wait,
                snapshot: ProfileSnapshotSpec::ProfileFile { uid: uid.clone() },
                runtime: RuntimePolicy::IfCurrentIs {
                    uid: uid.clone(),
                    options: RuntimeCommitOptions::apply_only(
                        crate::actions::runtime_apply::RuntimeApplyOptions::default(),
                    ),
                },
            },
            move |_before| async move {
                state
                    .store
                    .commit_remote_profile_update(&prepared.plan, prepared.remote)
                    .await
            },
        )
        .await?;

    let runtime = outcome.runtime;
    Ok(ProfileUpdateCommitResult {
        profiles: outcome.profiles,
        runtime_refresh: ProfileUpdateRuntimeRefresh {
            current_profile: runtime.is_some(),
            runtime_path: runtime.as_ref().map(|runtime| runtime.apply.runtime_path.clone()),
            runtime_validated: runtime.as_ref().is_some_and(|runtime| runtime.apply.runtime_validated),
            runtime_reloaded: runtime.as_ref().is_some_and(|runtime| runtime.apply.runtime_reloaded),
            warning: None,
        },
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
) -> Result<PreparedProfileUpdate> {
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
        let plan = state
            .store
            .prepare_remote_profile_update(uid, attempt_option.as_ref())
            .await?;
        match download_remote_profile(&plan.request).await {
            Ok(remote) => return Ok(PreparedProfileUpdate { plan, remote }),
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

    use clash_core::{
        IProfiles, PrfItem, PrfOption, RemoteProfileImport,
        config::{profiles::RemoteProfileDownload, store::RemoteProfileUpdatePlan},
    };

    use super::{
        PreparedProfileUpdate, ProfileUpdateRuntimeRefresh, commit_profile_update_transaction, due_profile_uids,
        profile_update_job_result, profile_update_success_message, redact_urls, subscription_profile_status,
        subscription_scheduler_status,
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
    async fn current_profile_update_transaction_regenerates_runtime_without_core() {
        let root = temp_root("subscription-current-runtime");
        let _ = fs::remove_dir_all(&root);
        install_fake_mihomo(&root);
        let options =
            ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options");
        let state = AppState::initialize(options).await.expect("state");

        let profiles = IProfiles {
            current: Some("Rcurrent".into()),
            items: Some(vec![PrfItem {
                uid: Some("Rcurrent".into()),
                itype: Some("remote".into()),
                name: Some("Current".into()),
                file: Some("Rcurrent.yaml".into()),
                url: Some("https://example.test/current".into()),
                ..PrfItem::default()
            }]),
        };
        profiles
            .save_file(&state.store.paths().profiles_config)
            .await
            .expect("profiles");
        tokio::fs::create_dir_all(&state.store.paths().profiles_dir)
            .await
            .expect("profiles dir");
        tokio::fs::write(
            state.store.paths().profiles_dir.join("Rcurrent.yaml"),
            "mode: global\nproxies: []\nproxy-groups: []\nrules: []\n",
        )
        .await
        .expect("old profile");
        let prepared = prepared_update(
            "Rcurrent",
            "https://example.test/current",
            r"
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
",
        );

        let commit = commit_profile_update_transaction(&state, "Rcurrent", prepared)
            .await
            .expect("update commit");
        let refresh = commit.runtime_refresh;

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
    async fn non_current_profile_update_transaction_does_not_regenerate_runtime() {
        let root = temp_root("subscription-non-current-runtime");
        let _ = fs::remove_dir_all(&root);
        let options =
            ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options");
        let state = AppState::initialize(options).await.expect("state");

        let profiles = IProfiles {
            current: Some("Rcurrent".into()),
            items: Some(vec![
                PrfItem {
                    uid: Some("Rcurrent".into()),
                    itype: Some("remote".into()),
                    name: Some("Current".into()),
                    file: Some("Rcurrent.yaml".into()),
                    url: Some("https://example.test/current".into()),
                    ..PrfItem::default()
                },
                PrfItem {
                    uid: Some("Rother".into()),
                    itype: Some("remote".into()),
                    name: Some("Other".into()),
                    file: Some("Rother.yaml".into()),
                    url: Some("https://example.test/other".into()),
                    ..PrfItem::default()
                },
            ]),
        };
        profiles
            .save_file(&state.store.paths().profiles_config)
            .await
            .expect("profiles");
        tokio::fs::create_dir_all(&state.store.paths().profiles_dir)
            .await
            .expect("profiles dir");
        tokio::fs::write(
            state.store.paths().profiles_dir.join("Rcurrent.yaml"),
            "mode: global\nproxies: []\nproxy-groups: []\nrules: []\n",
        )
        .await
        .expect("current profile");
        tokio::fs::write(
            state.store.paths().profiles_dir.join("Rother.yaml"),
            "mode: rule\nproxies: []\nproxy-groups: []\nrules: []\n",
        )
        .await
        .expect("other profile");
        let prepared = prepared_update(
            "Rother",
            "https://example.test/other",
            "mode: direct\nproxies: []\nproxy-groups: []\nrules: []\n",
        );

        let commit = commit_profile_update_transaction(&state, "Rother", prepared)
            .await
            .expect("update commit");

        assert_eq!(commit.runtime_refresh, ProfileUpdateRuntimeRefresh::default());
        assert!(!state.store.paths().runtime_config.exists());

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn current_profile_update_transaction_restores_file_when_runtime_generation_fails() {
        let root = temp_root("subscription-rollback-runtime-failure");
        let _ = fs::remove_dir_all(&root);
        install_fake_mihomo(&root);
        let options =
            ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options");
        let state = AppState::initialize(options).await.expect("state");
        let profiles = IProfiles {
            current: Some("Rbroken".into()),
            items: Some(vec![PrfItem {
                uid: Some("Rbroken".into()),
                itype: Some("remote".into()),
                name: Some("Broken".into()),
                file: Some("broken.yaml".into()),
                url: Some("https://example.test/broken".into()),
                ..PrfItem::default()
            }]),
        };
        profiles
            .save_file(&state.store.paths().profiles_config)
            .await
            .expect("profiles");
        tokio::fs::create_dir_all(&state.store.paths().profiles_dir)
            .await
            .expect("profiles dir");
        let old_profile = "mode: global\nproxies: []\nproxy-groups: []\nrules: []\n";
        tokio::fs::write(state.store.paths().profiles_dir.join("broken.yaml"), old_profile)
            .await
            .expect("old profile");
        let prepared = prepared_update("Rbroken", "https://example.test/broken", "proxy-groups: [");

        let err = commit_profile_update_transaction(&state, "Rbroken", prepared)
            .await
            .expect_err("runtime generation should fail");

        assert!(
            err.to_string().contains("subscription profile was rolled back"),
            "error should report rollback: {err:#}"
        );
        let restored = tokio::fs::read_to_string(state.store.paths().profiles_dir.join("broken.yaml"))
            .await
            .expect("restored profile");
        assert_eq!(restored, old_profile);

        let _ = fs::remove_dir_all(root);
    }

    fn prepared_update(uid: &str, url: &str, file_data: &str) -> PreparedProfileUpdate {
        PreparedProfileUpdate {
            plan: RemoteProfileUpdatePlan {
                uid: uid.into(),
                request: RemoteProfileImport {
                    url: url.into(),
                    uid: Some(uid.into()),
                    name: None,
                    desc: None,
                    option: None,
                },
                option: None,
            },
            remote: RemoteProfileDownload {
                url: url.into(),
                file_data: file_data.into(),
                name: "Updated".into(),
                extra: None,
                update_interval: None,
                home: None,
            },
        }
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
