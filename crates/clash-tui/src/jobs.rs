use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use clash_core::KernelSnapshot;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    sync::{Mutex, broadcast},
    task::AbortHandle,
};

const JOB_LIMIT: usize = 200;
const EVENT_CHANNEL_SIZE: usize = 256;
const PROFILE_UPDATE_JOB_KIND: &str = "profile-update";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JobStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRecord {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub target: Option<String>,
    pub status: JobStatus,
    pub message: Option<String>,
    pub error: Option<String>,
    pub result: Option<Value>,
    pub created_at: u64,
    pub updated_at: u64,
    pub finished_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClashTuiEvent {
    pub id: u64,
    pub timestamp: u64,
    #[serde(flatten)]
    pub payload: ClashTuiEventPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobCancelReport {
    pub job: JobRecord,
    pub supported: bool,
    pub cancelled: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "kebab-case")]
pub enum ClashTuiEventPayload {
    JobCreated { job: JobRecord },
    JobUpdated { job: JobRecord },
    KernelStateChanged { kernel: KernelSnapshot },
    MihomoTraffic { traffic: Value },
    MihomoLog { log: Value },
}

#[derive(Clone)]
pub struct JobManager {
    inner: Arc<Mutex<JobInner>>,
    sequence: Arc<AtomicU64>,
    events: broadcast::Sender<ClashTuiEvent>,
    history_path: Option<Arc<PathBuf>>,
    history_writable: Arc<AtomicBool>,
}

#[derive(Default)]
struct JobInner {
    jobs: HashMap<String, JobRecord>,
    order: VecDeque<String>,
    abort_handles: HashMap<String, AbortHandle>,
}

impl Default for JobManager {
    fn default() -> Self {
        let (events, _) = broadcast::channel(EVENT_CHANNEL_SIZE);
        Self {
            inner: Arc::new(Mutex::new(JobInner::default())),
            sequence: Arc::new(AtomicU64::new(1)),
            events,
            history_path: None,
            history_writable: Arc::new(AtomicBool::new(true)),
        }
    }
}

impl JobManager {
    pub async fn with_history_file(path: PathBuf) -> Self {
        let manager = Self {
            history_path: Some(Arc::new(path)),
            ..Self::default()
        };
        manager.load_history_best_effort().await;
        manager
    }

    pub async fn create_unique_active(
        &self,
        kind: impl Into<String>,
        name: impl Into<String>,
        target: Option<String>,
    ) -> (JobRecord, bool) {
        let kind = kind.into();
        let name = name.into();
        let mut inner = self.inner.lock().await;
        if let Some(job) = active_job(&inner, &kind, target.as_deref()) {
            return (job, false);
        }

        let id = self.next_job_id();
        let now = current_timestamp_secs();
        let job = JobRecord {
            id,
            kind,
            name,
            target,
            status: JobStatus::Pending,
            message: None,
            error: None,
            result: None,
            created_at: now,
            updated_at: now,
            finished_at: None,
        };
        insert_job_locked(&mut inner, job.clone());
        drop(inner);

        self.persist_history_best_effort().await;
        self.emit(ClashTuiEventPayload::JobCreated { job: job.clone() });
        (job, true)
    }

    pub async fn start(&self, id: &str, message: impl Into<String>) -> Option<JobRecord> {
        self.update_job(id, |job| {
            if matches!(job.status, JobStatus::Pending | JobStatus::Running) {
                job.status = JobStatus::Running;
                job.message = Some(message.into());
            }
        })
        .await
    }

    pub async fn progress(&self, id: &str, message: impl Into<String>) -> Option<JobRecord> {
        self.update_job(id, |job| {
            if matches!(job.status, JobStatus::Pending | JobStatus::Running) {
                job.message = Some(message.into());
            }
        })
        .await
    }

    pub async fn finish(&self, id: &str, message: impl Into<String>, result: Option<Value>) -> Option<JobRecord> {
        self.update_job(id, |job| {
            if matches!(job.status, JobStatus::Cancelled) {
                return;
            }
            let now = current_timestamp_secs();
            job.status = JobStatus::Succeeded;
            job.message = Some(message.into());
            job.result = result;
            job.finished_at = Some(now);
        })
        .await
        .inspect(|job| {
            if matches!(
                job.status,
                JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled
            ) {
                let jobs = self.clone();
                let id = job.id.clone();
                tokio::spawn(async move {
                    jobs.remove_abort_handle(&id).await;
                });
            }
        })
    }

    pub async fn fail(&self, id: &str, message: impl Into<String>) -> Option<JobRecord> {
        self.update_job(id, |job| {
            if matches!(job.status, JobStatus::Cancelled) {
                return;
            }
            let now = current_timestamp_secs();
            let message = message.into();
            job.status = JobStatus::Failed;
            job.message = Some("job failed".into());
            job.error = Some(message);
            job.finished_at = Some(now);
        })
        .await
        .inspect(|job| {
            if matches!(
                job.status,
                JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled
            ) {
                let jobs = self.clone();
                let id = job.id.clone();
                tokio::spawn(async move {
                    jobs.remove_abort_handle(&id).await;
                });
            }
        })
    }

    pub async fn list(&self) -> Vec<JobRecord> {
        let inner = self.inner.lock().await;
        inner
            .order
            .iter()
            .filter_map(|id| inner.jobs.get(id))
            .cloned()
            .collect()
    }

    pub async fn get(&self, id: &str) -> Option<JobRecord> {
        self.inner.lock().await.jobs.get(id).cloned()
    }

    pub async fn register_abort_handle(&self, id: &str, abort_handle: AbortHandle) -> bool {
        let mut inner = self.inner.lock().await;
        let Some(job) = inner.jobs.get(id) else {
            abort_handle.abort();
            return false;
        };
        if !matches!(job.status, JobStatus::Pending | JobStatus::Running) {
            abort_handle.abort();
            return false;
        }
        inner.abort_handles.insert(id.to_owned(), abort_handle);
        true
    }

    pub async fn cancel_report(&self, id: &str) -> Option<JobCancelReport> {
        let mut abort_handle = None;
        let mut cancelled_job = None;
        let report = {
            let mut inner = self.inner.lock().await;
            let status = inner.jobs.get(id)?.status;
            match status {
                JobStatus::Pending | JobStatus::Running => {
                    if let Some(handle) = inner.abort_handles.remove(id) {
                        abort_handle = Some(handle);
                        let job = inner.jobs.get_mut(id)?;
                        let now = current_timestamp_secs();
                        job.status = JobStatus::Cancelled;
                        job.message = Some("任务已取消".into());
                        job.error = None;
                        job.finished_at = Some(now);
                        job.updated_at = now;
                        let job = job.clone();
                        cancelled_job = Some(job.clone());
                        JobCancelReport {
                            job,
                            supported: true,
                            cancelled: true,
                            message: "已取消任务，执行已终止".into(),
                        }
                    } else {
                        JobCancelReport {
                            job: inner.jobs.get(id)?.clone(),
                            supported: false,
                            cancelled: false,
                            message: "该任务当前没有可取消执行句柄，可能来自历史记录或已在其他进程中执行".into(),
                        }
                    }
                }
                JobStatus::Succeeded | JobStatus::Failed | JobStatus::Cancelled => JobCancelReport {
                    job: inner.jobs.get(id)?.clone(),
                    supported: false,
                    cancelled: false,
                    message: "任务已经结束，无需取消；原任务状态未改变".into(),
                },
            }
        };
        if let Some(handle) = abort_handle {
            handle.abort();
        }
        if let Some(job) = cancelled_job {
            self.persist_history_best_effort().await;
            self.emit(ClashTuiEventPayload::JobUpdated { job });
        }
        Some(report)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ClashTuiEvent> {
        self.events.subscribe()
    }

    pub fn emit_kernel_state_changed(&self, kernel: KernelSnapshot) {
        self.emit(ClashTuiEventPayload::KernelStateChanged { kernel });
    }

    async fn update_job(&self, id: &str, update: impl FnOnce(&mut JobRecord)) -> Option<JobRecord> {
        let mut inner = self.inner.lock().await;
        let job = inner.jobs.get_mut(id)?;
        update(job);
        job.updated_at = current_timestamp_secs();
        let job = job.clone();
        drop(inner);

        self.persist_history_best_effort().await;
        self.emit(ClashTuiEventPayload::JobUpdated { job: job.clone() });
        Some(job)
    }

    fn emit(&self, payload: ClashTuiEventPayload) {
        let event = ClashTuiEvent {
            id: self.sequence.fetch_add(1, Ordering::Relaxed),
            timestamp: current_timestamp_secs(),
            payload,
        };
        let _ = self.events.send(event);
    }

    fn next_job_id(&self) -> String {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        format!("job-{}-{sequence}", current_timestamp_secs())
    }

    async fn remove_abort_handle(&self, id: &str) {
        self.inner.lock().await.abort_handles.remove(id);
    }

    async fn load_history_best_effort(&self) {
        let Some(path) = self.history_path.as_ref() else {
            return;
        };
        let bytes = match tokio::fs::read(path.as_ref()).await {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                self.disable_history_persistence();
                return;
            }
            Err(err) => {
                eprintln!("failed to read job history {}: {err}", path.display());
                return;
            }
        };
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return;
        }
        let mut jobs = match serde_json::from_slice::<Vec<JobRecord>>(&bytes) {
            Ok(jobs) => jobs,
            Err(err) => {
                eprintln!("failed to parse job history {}: {err}", path.display());
                return;
            }
        };
        if jobs.len() > JOB_LIMIT {
            jobs = jobs.split_off(jobs.len() - JOB_LIMIT);
        }

        let mut inner = self.inner.lock().await;
        inner.jobs.clear();
        inner.order.clear();
        let mut rewrite_history = false;
        let now = current_timestamp_secs();
        for mut job in jobs {
            rewrite_history |= sanitize_job_for_history(&mut job, now);
            insert_job_locked(&mut inner, job);
        }
        drop(inner);

        if rewrite_history {
            self.persist_history_best_effort().await;
        }
    }

    async fn persist_history_best_effort(&self) {
        let Some(path) = self.history_path.as_ref() else {
            return;
        };
        if !self.history_writable.load(Ordering::Relaxed) {
            return;
        }
        let jobs = self.list().await;
        let Some(parent) = path.parent() else {
            eprintln!(
                "failed to persist job history {}: missing parent directory",
                path.display()
            );
            return;
        };
        if let Err(err) = tokio::fs::create_dir_all(parent).await {
            if err.kind() == std::io::ErrorKind::PermissionDenied {
                self.disable_history_persistence();
                return;
            }
            eprintln!("failed to create job history directory {}: {err}", parent.display());
            return;
        }
        let bytes = match serde_json::to_vec_pretty(&jobs) {
            Ok(bytes) => bytes,
            Err(err) => {
                eprintln!("failed to serialize job history {}: {err}", path.display());
                return;
            }
        };
        let tmp_path = history_temp_path(path.as_ref());
        if let Err(err) = tokio::fs::write(&tmp_path, bytes).await {
            if err.kind() == std::io::ErrorKind::PermissionDenied {
                self.disable_history_persistence();
                return;
            }
            eprintln!("failed to write job history {}: {err}", tmp_path.display());
            return;
        }
        if let Err(err) = tokio::fs::rename(&tmp_path, path.as_ref()).await {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            if err.kind() == std::io::ErrorKind::PermissionDenied {
                self.disable_history_persistence();
                return;
            }
            eprintln!("failed to replace job history {}: {err}", path.display());
        }
    }

    fn disable_history_persistence(&self) {
        self.history_writable.store(false, Ordering::Relaxed);
    }
}

fn history_temp_path(path: &Path) -> PathBuf {
    let file_name = path.file_name().and_then(|name| name.to_str()).unwrap_or("jobs.json");
    path.with_file_name(format!(
        "{file_name}.tmp.{}-{}",
        std::process::id(),
        current_timestamp_secs()
    ))
}

fn active_job(inner: &JobInner, kind: &str, target: Option<&str>) -> Option<JobRecord> {
    inner
        .order
        .iter()
        .rev()
        .filter_map(|id| inner.jobs.get(id))
        .find(|job| {
            job.kind == kind
                && job.target.as_deref() == target
                && matches!(job.status, JobStatus::Pending | JobStatus::Running)
        })
        .cloned()
}

fn insert_job_locked(inner: &mut JobInner, job: JobRecord) {
    if inner.order.len() >= JOB_LIMIT
        && let Some(oldest) = inner.order.pop_front()
    {
        inner.jobs.remove(&oldest);
        inner.abort_handles.remove(&oldest);
    }
    inner.order.push_back(job.id.clone());
    inner.jobs.insert(job.id.clone(), job);
}

fn sanitize_job_for_history(job: &mut JobRecord, now: u64) -> bool {
    let interrupted = matches!(job.status, JobStatus::Pending | JobStatus::Running);
    if interrupted {
        job.status = JobStatus::Cancelled;
        job.message = Some("任务随上次进程退出中断，可重试".into());
        job.error = None;
        job.updated_at = now;
        job.finished_at = Some(now);
    }
    let redacted = if job.kind == PROFILE_UPDATE_JOB_KIND
        && let Some(result) = job.result.as_ref()
        && value_contains_http_url(result)
    {
        job.result = Some(serde_json::json!({
            "uid": job.target.as_deref(),
        }));
        true
    } else {
        false
    };
    interrupted || redacted
}

fn value_contains_http_url(value: &Value) -> bool {
    match value {
        Value::String(value) => value.contains("http://") || value.contains("https://"),
        Value::Array(values) => values.iter().any(value_contains_http_url),
        Value::Object(values) => values.values().any(value_contains_http_url),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
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
    #![allow(clippy::expect_used)]

    use std::path::PathBuf;

    use super::{JobManager, JobRecord, JobStatus, current_timestamp_secs};

    #[tokio::test]
    async fn job_manager_lists_gets_and_reports_missing_cancel_handle() {
        let jobs = JobManager::default();
        let (job, created) = jobs
            .create_unique_active("profile-update", "Update remote profile R001", Some("R001".into()))
            .await;

        assert!(created);
        assert_eq!(jobs.list().await.len(), 1);
        assert_eq!(jobs.get(&job.id).await.expect("job").status, JobStatus::Pending);

        let report = jobs.cancel_report(&job.id).await.expect("cancel report");

        assert!(!report.supported);
        assert!(!report.cancelled);
        assert!(report.message.contains("没有可取消执行句柄"));
    }

    #[tokio::test]
    async fn job_manager_cancels_active_job_with_abort_handle() {
        let jobs = JobManager::default();
        let (job, created) = jobs
            .create_unique_active("profile-update", "Update remote profile R001", Some("R001".into()))
            .await;
        assert!(created);
        jobs.start(&job.id, "downloading").await;
        let handle = tokio::spawn(async {
            std::future::pending::<()>().await;
        });
        assert!(jobs.register_abort_handle(&job.id, handle.abort_handle()).await);

        let report = jobs.cancel_report(&job.id).await.expect("cancel report");
        let cancelled = jobs.get(&job.id).await.expect("cancelled job");

        assert!(report.supported);
        assert!(report.cancelled);
        assert_eq!(report.job.status, JobStatus::Cancelled);
        assert_eq!(cancelled.status, JobStatus::Cancelled);
        assert_eq!(cancelled.finished_at, Some(cancelled.updated_at));
        assert!(handle.await.expect_err("task should be aborted").is_cancelled());
    }

    #[tokio::test]
    async fn cancelled_job_is_not_overwritten_by_late_finish() {
        let jobs = JobManager::default();
        let (job, created) = jobs
            .create_unique_active("profile-update", "Update remote profile R001", Some("R001".into()))
            .await;
        assert!(created);
        jobs.start(&job.id, "downloading").await;
        let handle = tokio::spawn(async {
            std::future::pending::<()>().await;
        });
        assert!(jobs.register_abort_handle(&job.id, handle.abort_handle()).await);
        let report = jobs.cancel_report(&job.id).await.expect("cancel report");
        assert!(report.cancelled);

        jobs.finish(&job.id, "profile updated", Some(serde_json::json!({"ok": true})))
            .await;

        let cancelled = jobs.get(&job.id).await.expect("cancelled job");
        assert_eq!(cancelled.status, JobStatus::Cancelled);
        assert_eq!(cancelled.result, None);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn job_manager_persists_history_between_instances() {
        let path = history_test_path();
        let jobs = JobManager::with_history_file(path.clone()).await;
        let (job, created) = jobs
            .create_unique_active("profile-update", "Update remote profile R001", Some("R001".into()))
            .await;

        assert!(created);
        jobs.start(&job.id, "downloading").await;
        jobs.finish(&job.id, "profile updated", Some(serde_json::json!({"ok": true})))
            .await;

        let restored = JobManager::with_history_file(path.clone()).await;
        let restored_job = restored.get(&job.id).await.expect("persisted job");

        assert_eq!(restored_job.status, JobStatus::Succeeded);
        assert_eq!(restored_job.message.as_deref(), Some("profile updated"));

        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn job_manager_treats_empty_history_as_no_history() {
        let path = history_test_path();
        tokio::fs::write(&path, b"").await.expect("write empty history");

        let jobs = JobManager::with_history_file(path.clone()).await;
        assert!(jobs.list().await.is_empty());

        let (job, created) = jobs
            .create_unique_active("profile-update", "Update remote profile R001", Some("R001".into()))
            .await;
        assert!(created);
        jobs.finish(&job.id, "profile updated", None).await;

        let restored = JobManager::with_history_file(path.clone()).await;
        assert_eq!(
            restored.get(&job.id).await.expect("persisted job").status,
            JobStatus::Succeeded
        );

        let _ = tokio::fs::remove_file(path).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn job_manager_disables_unwritable_history_without_losing_jobs() {
        use std::{os::unix::fs::PermissionsExt as _, sync::atomic::Ordering};

        let dir = history_test_dir();
        tokio::fs::create_dir_all(&dir).await.expect("create history dir");
        let mut permissions = tokio::fs::metadata(&dir)
            .await
            .expect("history dir metadata")
            .permissions();
        permissions.set_mode(0o555);
        tokio::fs::set_permissions(&dir, permissions)
            .await
            .expect("make history dir readonly");

        let path = dir.join("jobs.json");
        let jobs = JobManager::with_history_file(path.clone()).await;
        let (job, created) = jobs
            .create_unique_active("runtime-config-validate", "Validate runtime config", None)
            .await;

        if jobs.history_writable.load(Ordering::Relaxed) {
            restore_history_test_dir(&dir).await;
            return;
        }

        assert!(created);
        jobs.finish(&job.id, "runtime config is valid", None).await;

        let finished = jobs.get(&job.id).await.expect("finished job");
        assert_eq!(finished.status, JobStatus::Succeeded);
        assert!(!path.exists());

        restore_history_test_dir(&dir).await;
    }

    #[tokio::test]
    async fn job_manager_sanitizes_legacy_profile_update_history() {
        let path = history_test_path();
        let now = current_timestamp_secs();
        let legacy = JobRecord {
            id: "job-legacy-1".into(),
            kind: "profile-update".into(),
            name: "Update remote profile R001".into(),
            target: Some("R001".into()),
            status: JobStatus::Succeeded,
            message: Some("profile updated".into()),
            error: None,
            result: Some(serde_json::json!({
                "items": [
                    {
                        "uid": "R001",
                        "url": "https://example.test/sub"
                    }
                ]
            })),
            created_at: now,
            updated_at: now,
            finished_at: Some(now),
        };
        let bytes = serde_json::to_vec(&vec![legacy]).expect("legacy jobs");
        tokio::fs::write(&path, bytes).await.expect("write legacy jobs");

        let jobs = JobManager::with_history_file(path.clone()).await;
        let restored = jobs.get("job-legacy-1").await.expect("legacy job");
        let restored_text = serde_json::to_string(&restored).unwrap_or_default();
        let file_text = tokio::fs::read_to_string(&path).await.unwrap_or_default();

        assert_eq!(
            restored.result.and_then(|value| value.get("uid").cloned()),
            Some(serde_json::json!("R001"))
        );
        assert!(!restored_text.contains("https://"));
        assert!(!file_text.contains("https://"));

        let _ = tokio::fs::remove_file(path).await;
    }

    #[tokio::test]
    async fn job_manager_marks_interrupted_history_as_cancelled() {
        let path = history_test_path();
        let now = current_timestamp_secs();
        let interrupted = JobRecord {
            id: "job-interrupted-1".into(),
            kind: "profile-update".into(),
            name: "Update remote profile R001".into(),
            target: Some("R001".into()),
            status: JobStatus::Running,
            message: Some("downloading".into()),
            error: None,
            result: None,
            created_at: now,
            updated_at: now,
            finished_at: None,
        };
        let bytes = serde_json::to_vec(&vec![interrupted]).expect("interrupted jobs");
        tokio::fs::write(&path, bytes).await.expect("write interrupted jobs");

        let jobs = JobManager::with_history_file(path.clone()).await;
        let restored = jobs.get("job-interrupted-1").await.expect("interrupted job");

        assert_eq!(restored.status, JobStatus::Cancelled);
        assert_eq!(restored.message.as_deref(), Some("任务随上次进程退出中断，可重试"));
        assert!(restored.finished_at.is_some());

        let _ = tokio::fs::remove_file(path).await;
    }

    fn history_test_path() -> PathBuf {
        let mut path = std::env::temp_dir();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        path.push(format!("clash-tui-jobs-{}-{timestamp}.json", std::process::id(),));
        path
    }

    fn history_test_dir() -> PathBuf {
        let mut path = std::env::temp_dir();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        path.push(format!("clash-tui-jobs-{}-{timestamp}", std::process::id(),));
        path
    }

    #[cfg(unix)]
    async fn restore_history_test_dir(dir: &PathBuf) {
        use std::os::unix::fs::PermissionsExt as _;

        if let Ok(metadata) = tokio::fs::metadata(dir).await {
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o755);
            let _ = tokio::fs::set_permissions(dir, permissions).await;
        }
        let _ = tokio::fs::remove_dir_all(dir).await;
    }
}
