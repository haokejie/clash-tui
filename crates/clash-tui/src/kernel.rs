use std::{
    collections::VecDeque,
    fs::OpenOptions,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context as _, Result, anyhow};
use clash_core::{
    AppPaths, IAppSettings, KernelOwner, KernelSnapshot, KernelState, OperationStatus, RuntimeConfigGenerator,
};
use clash_mihomo::{MihomoClient as _, MihomoClientConfig, SimpleMihomoClient};
use serde::{Deserialize, Serialize};
use tokio::{
    process::{Child, Command},
    sync::Mutex,
    time::{sleep, timeout},
};

use crate::{jobs::JobManager, options::ClashTuiOptions};

#[cfg(not(unix))]
compile_error!("clash-tui requires Unix domain sockets for the mihomo controller");

const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(5);
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(2);
const LOCAL_VERSION_TIMEOUT: Duration = Duration::from_secs(5);
const HEALTH_FAILURE_THRESHOLD: u8 = 3;
const LOG_LIMIT: usize = 200;
const MIHOMO_LOG_FILE: &str = "mihomo.log";
const OWNER_FILE: &str = "mihomo-owner.json";
const JOB_START: &str = "kernel-start";
const JOB_STOP: &str = "kernel-stop";
const JOB_RESTART: &str = "kernel-restart";
pub const ENV_CORE_OWNER: &str = "CLASH_TUI_CORE_OWNER";
pub const ENV_SERVICE_NAME: &str = "CLASH_TUI_SERVICE_NAME";
pub const DEFAULT_SYSTEMD_SERVICE_NAME: &str = "clash-tui.service";

#[derive(Debug, Clone)]
pub struct KernelProcessConfig {
    pub mihomo_bin: PathBuf,
    pub home_dir: PathBuf,
    pub resource_dir: PathBuf,
    pub ipc_path: PathBuf,
    pub pid_path: PathBuf,
    pub owner_path: PathBuf,
    pub secret: Option<String>,
}

impl KernelProcessConfig {
    #[must_use]
    pub fn from_options(options: &ClashTuiOptions, paths: &AppPaths, secret: Option<String>) -> Self {
        Self {
            mihomo_bin: options.resolved_mihomo_bin(paths),
            home_dir: paths.home_dir.clone(),
            resource_dir: paths.resources_dir.clone(),
            ipc_path: paths.ipc_path.clone(),
            pid_path: paths.home_dir.join("mihomo.pid"),
            owner_path: paths.home_dir.join(OWNER_FILE),
            secret,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OwnerMarker {
    pid: u32,
    owner: KernelOwner,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

#[derive(Clone)]
pub struct KernelManager {
    config: KernelProcessConfig,
    runtime: RuntimeConfigGenerator,
    health_client: SimpleMihomoClient,
    inner: Arc<Mutex<KernelInner>>,
    operation: Arc<Mutex<()>>,
    events: Option<JobManager>,
}

struct KernelInner {
    snapshot: KernelSnapshot,
    child: Option<Child>,
    current_job: Option<String>,
    health_failures: u8,
    logs: VecDeque<String>,
}

impl Default for KernelInner {
    fn default() -> Self {
        Self {
            snapshot: KernelSnapshot::stopped(),
            child: None,
            current_job: None,
            health_failures: 0,
            logs: VecDeque::with_capacity(LOG_LIMIT),
        }
    }
}

impl KernelManager {
    #[must_use]
    #[cfg(test)]
    pub fn new(config: KernelProcessConfig, runtime: RuntimeConfigGenerator) -> Self {
        Self::new_with_events(config, runtime, None)
    }

    #[must_use]
    pub fn with_events(config: KernelProcessConfig, runtime: RuntimeConfigGenerator, events: JobManager) -> Self {
        Self::new_with_events(config, runtime, Some(events))
    }

    fn new_with_events(
        config: KernelProcessConfig,
        runtime: RuntimeConfigGenerator,
        events: Option<JobManager>,
    ) -> Self {
        let health_client = SimpleMihomoClient::new(health_client_config(&config));
        Self {
            config,
            runtime,
            health_client,
            inner: Arc::new(Mutex::new(KernelInner::default())),
            operation: Arc::new(Mutex::new(())),
            events,
        }
    }

    pub fn spawn_health_monitor(&self) {
        let manager = self.clone();
        tokio::spawn(async move {
            loop {
                sleep(HEALTH_CHECK_INTERVAL).await;
                manager.check_health_once().await;
            }
        });
    }

    pub async fn snapshot(&self) -> KernelSnapshot {
        self.refresh_child_exit().await
    }

    pub async fn external_snapshot(&self) -> KernelSnapshot {
        let snapshot = self.snapshot().await;
        if !matches!(snapshot.state, KernelState::Stopped | KernelState::Crashed) {
            return snapshot;
        }

        let Some(pid) = self.read_pid_file().await else {
            return self.stopped_snapshot().await;
        };

        if !process_exists(pid) {
            let _ = self.remove_runtime_markers().await;
            return self.stopped_snapshot().await;
        }

        match self.health_client.version().await {
            Ok(version) => {
                let (owner, owner_detail) = self.owner_for_pid(pid).await;
                KernelSnapshot {
                    state: KernelState::Running,
                    owner,
                    owner_detail,
                    pid: Some(pid),
                    version: Some(version.version),
                    last_error: None,
                    last_exit: None,
                }
            }
            Err(err) => {
                let (owner, owner_detail) = self.owner_for_pid(pid).await;
                KernelSnapshot {
                    state: KernelState::Unhealthy,
                    owner,
                    owner_detail,
                    pid: Some(pid),
                    version: None,
                    last_error: Some(err.to_string()),
                    last_exit: None,
                }
            }
        }
    }

    pub async fn check_health_once(&self) -> KernelSnapshot {
        self.refresh_child_exit().await;
        if !self.should_probe_health().await {
            return self.snapshot().await;
        }

        match self.health_client.version().await {
            Ok(version) => self.record_health_success(version.version).await,
            Err(err) => self.record_health_failure(err.to_string()).await,
        }
    }

    pub async fn logs(&self) -> Vec<String> {
        let persisted = self.read_persisted_logs().await.unwrap_or_default();
        let memory = self.inner.lock().await.logs.iter().cloned().collect::<Vec<_>>();
        merge_logs(persisted, memory)
    }

    pub async fn clear_logs(&self) -> Result<()> {
        tokio::fs::create_dir_all(self.log_dir())
            .await
            .with_context(|| format!("failed to create {}", self.log_dir().display()))?;
        tokio::fs::write(self.mihomo_log_path(), "")
            .await
            .with_context(|| format!("failed to clear {}", self.mihomo_log_path().display()))?;
        self.inner.lock().await.logs.clear();
        Ok(())
    }

    pub async fn start(&self) -> Result<OperationStatus> {
        let Ok(_operation_guard) = self.operation.try_lock() else {
            return Ok(self.busy_status().await);
        };

        self.refresh_child_exit().await;
        if matches!(
            self.snapshot_state().await,
            KernelState::Running | KernelState::Starting | KernelState::Restarting | KernelState::Unhealthy
        ) {
            return Ok(self.status(false, None).await);
        }
        if let Some(snapshot) = self.external_running_snapshot().await {
            let state = snapshot.state;
            let owner = snapshot.owner;
            self.adopt_external_snapshot(snapshot).await;
            return Ok(OperationStatus {
                accepted: false,
                current_job: None,
                state,
                owner,
                message: Some("Core 已在运行，未重复启动".into()),
            });
        }

        self.set_job_state(JOB_START, KernelState::Starting, None).await;
        match self.spawn_child().await {
            Ok((mut child, pid)) => {
                let snapshot = KernelSnapshot {
                    state: KernelState::Running,
                    owner: KernelOwner::Detached,
                    owner_detail: None,
                    pid,
                    version: None,
                    last_error: None,
                    last_exit: None,
                };
                if let Err(err) = self.write_pid_file(pid).await {
                    self.cleanup_spawned_child(&mut child, pid, "failed-start").await;
                    return Err(err);
                }
                if let Some(pid) = pid
                    && let Err(err) = self.write_owner_marker(pid, KernelOwner::Detached, None).await
                {
                    self.cleanup_spawned_child(&mut child, Some(pid), "failed-start").await;
                    return Err(err);
                }
                let mut inner = self.inner.lock().await;
                inner.child = Some(child);
                inner.snapshot = snapshot.clone();
                inner.health_failures = 0;
                inner.current_job = None;
                drop(inner);
                self.emit_kernel_state_changed(snapshot);
                Ok(OperationStatus {
                    accepted: true,
                    current_job: None,
                    state: KernelState::Running,
                    owner: KernelOwner::Detached,
                    message: None,
                })
            }
            Err(err) => {
                let message = err.to_string();
                self.set_stopped_after_failure(message.clone()).await;
                Err(anyhow!(message))
            }
        }
    }

    pub async fn stop(&self) -> Result<OperationStatus> {
        let Ok(_operation_guard) = self.operation.try_lock() else {
            return Ok(self.busy_status().await);
        };

        self.refresh_child_exit().await;
        self.set_job_state(JOB_STOP, KernelState::Stopping, None).await;
        self.stop_child("stopped").await?;
        Ok(OperationStatus {
            accepted: true,
            current_job: None,
            state: KernelState::Stopped,
            owner: KernelOwner::Stopped,
            message: None,
        })
    }

    pub async fn restart(&self) -> Result<OperationStatus> {
        let Ok(_operation_guard) = self.operation.try_lock() else {
            return Ok(self.busy_status().await);
        };

        self.refresh_child_exit().await;
        self.set_job_state(JOB_RESTART, KernelState::Restarting, None).await;
        self.stop_child("restarting").await?;

        match self.spawn_child().await {
            Ok((mut child, pid)) => {
                let snapshot = KernelSnapshot {
                    state: KernelState::Running,
                    owner: KernelOwner::Detached,
                    owner_detail: None,
                    pid,
                    version: None,
                    last_error: None,
                    last_exit: None,
                };
                if let Err(err) = self.write_pid_file(pid).await {
                    self.cleanup_spawned_child(&mut child, pid, "failed-restart").await;
                    return Err(err);
                }
                if let Some(pid) = pid
                    && let Err(err) = self.write_owner_marker(pid, KernelOwner::Detached, None).await
                {
                    self.cleanup_spawned_child(&mut child, Some(pid), "failed-restart")
                        .await;
                    return Err(err);
                }
                let mut inner = self.inner.lock().await;
                inner.child = Some(child);
                inner.snapshot = snapshot.clone();
                inner.health_failures = 0;
                inner.current_job = None;
                drop(inner);
                self.emit_kernel_state_changed(snapshot);
                Ok(OperationStatus {
                    accepted: true,
                    current_job: None,
                    state: KernelState::Running,
                    owner: KernelOwner::Detached,
                    message: None,
                })
            }
            Err(err) => {
                let message = err.to_string();
                self.set_stopped_after_failure(message.clone()).await;
                Err(anyhow!(message))
            }
        }
    }

    pub async fn run_foreground(&self, owner: KernelOwner, owner_detail: Option<String>) -> Result<()> {
        let Ok(_operation_guard) = self.operation.try_lock() else {
            anyhow::bail!("Core 正忙，无法以前台方式启动");
        };

        self.refresh_child_exit().await;
        if let Some(snapshot) = self.external_running_snapshot().await {
            anyhow::bail!("Core 已在运行（owner: {:?}, pid: {:?}）", snapshot.owner, snapshot.pid);
        }

        self.set_job_state(JOB_START, KernelState::Starting, None).await;
        let (mut child, pid) = match self.spawn_child().await {
            Ok(result) => result,
            Err(err) => {
                let message = err.to_string();
                self.set_stopped_after_failure(message.clone()).await;
                return Err(anyhow!(message));
            }
        };
        if let Err(err) = self.write_pid_file(pid).await {
            self.cleanup_spawned_child(&mut child, pid, "failed-run").await;
            return Err(err);
        }
        if let Some(pid) = pid
            && let Err(err) = self.write_owner_marker(pid, owner, owner_detail.clone()).await
        {
            self.cleanup_spawned_child(&mut child, Some(pid), "failed-run").await;
            return Err(err);
        }

        let snapshot = KernelSnapshot {
            state: KernelState::Running,
            owner,
            owner_detail: owner_detail.clone(),
            pid,
            version: None,
            last_error: None,
            last_exit: None,
        };
        {
            let mut inner = self.inner.lock().await;
            inner.snapshot = snapshot.clone();
            inner.health_failures = 0;
            inner.current_job = None;
        }
        self.emit_kernel_state_changed(snapshot);

        let result = wait_for_child_or_shutdown(&mut child).await;
        match result {
            ForegroundExit::Process(status) => {
                let _ = self.remove_runtime_markers().await;
                self.set_stopped(Some(status.to_string()), None).await;
                if status.success() {
                    Ok(())
                } else {
                    anyhow::bail!("mihomo exited: {status}")
                }
            }
            ForegroundExit::Shutdown => {
                if let Some(pid) = pid {
                    self.signal_and_wait_child(&mut child, pid, "stopped").await?;
                } else {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    self.set_stopped(Some("stopped".into()), None).await;
                }
                let _ = self.remove_runtime_markers().await;
                Ok(())
            }
            ForegroundExit::WaitError(err) => {
                let message = format!("failed to wait mihomo process: {err}");
                let _ = self.remove_runtime_markers().await;
                self.set_unhealthy(message.clone()).await;
                Err(anyhow!(message))
            }
        }
    }

    async fn spawn_child(&self) -> Result<(Child, Option<u32>)> {
        let runtime = self
            .runtime
            .generate()
            .await
            .context("failed to generate runtime config")?;
        let runtime_path = PathBuf::from(&runtime.path);
        if path_requires_existing_file(&self.config.mihomo_bin) && !self.config.mihomo_bin.is_file() {
            return Err(anyhow!(
                "mihomo binary not found: {}; set --mihomo-bin or CLASH_TUI_MIHOMO_BIN",
                self.config.mihomo_bin.display()
            ));
        }
        self.sync_geo_resources().await?;
        self.prepare_ipc_path().await?;
        let core_log_enabled = self.core_log_enabled().await;
        let (stdout, stderr) = if core_log_enabled {
            (self.open_mihomo_log_stdio()?, self.open_mihomo_log_stdio()?)
        } else {
            (Stdio::null(), Stdio::null())
        };

        let mut command = Command::new(&self.config.mihomo_bin);
        command
            .arg("-d")
            .arg(&self.config.home_dir)
            .arg("-f")
            .arg(&runtime_path)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .kill_on_drop(false);

        #[cfg(unix)]
        {
            command.arg("-ext-ctl-unix").arg(&self.config.ipc_path);
        }

        let child = command
            .spawn()
            .with_context(|| format!("failed to spawn mihomo: {}", self.config.mihomo_bin.display()))?;
        let pid = child.id();

        Ok((child, pid))
    }

    async fn prepare_ipc_path(&self) -> Result<()> {
        #[cfg(unix)]
        {
            if let Some(parent) = self.config.ipc_path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .with_context(|| format!("failed to create mihomo IPC directory: {}", parent.display()))?;
            }
            if tokio::fs::metadata(&self.config.ipc_path).await.is_ok() {
                let _ = tokio::fs::remove_file(&self.config.ipc_path).await;
            }
        }

        Ok(())
    }

    fn open_mihomo_log_stdio(&self) -> Result<Stdio> {
        std::fs::create_dir_all(self.log_dir())
            .with_context(|| format!("failed to create mihomo log directory: {}", self.log_dir().display()))?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.mihomo_log_path())
            .with_context(|| format!("failed to open mihomo log file: {}", self.mihomo_log_path().display()))?;
        Ok(Stdio::from(file))
    }

    async fn core_log_enabled(&self) -> bool {
        let settings_path = self.config.home_dir.join("settings.yaml");
        IAppSettings::load_or_default(settings_path)
            .await
            .enable_core_log
            .unwrap_or(false)
    }

    async fn sync_geo_resources(&self) -> Result<()> {
        tokio::fs::create_dir_all(&self.config.home_dir)
            .await
            .with_context(|| {
                format!(
                    "failed to create mihomo home directory: {}",
                    self.config.home_dir.display()
                )
            })?;

        for file_name in GEO_RESOURCE_FILES {
            let source = self.config.resource_dir.join(file_name);
            if !source.is_file() {
                continue;
            }

            let target = self.config.home_dir.join(file_name);
            if target.exists() {
                continue;
            }

            tokio::fs::copy(&source, &target).await.with_context(|| {
                format!(
                    "failed to copy mihomo geo resource {} to {}",
                    source.display(),
                    target.display()
                )
            })?;
            self.append_kernel_log(format!(
                "synced mihomo geo resource {} from {}",
                file_name,
                self.config.resource_dir.display()
            ))
            .await;
        }

        Ok(())
    }

    async fn stop_child(&self, exit_message: &'static str) -> Result<()> {
        let child = {
            let mut inner = self.inner.lock().await;
            inner.child.take()
        };

        if let Some(mut child) = child {
            if let Some(pid) = child.id() {
                let _ = signal_process(pid, libc::SIGINT);
            } else {
                let _ = child.start_kill();
            }
            match timeout(STOP_TIMEOUT, child.wait()).await {
                Ok(Ok(status)) => {
                    self.set_stopped(Some(format!("{exit_message}: {status}")), None).await;
                }
                Ok(Err(err)) => {
                    let message = format!("failed to wait mihomo process: {err}");
                    self.set_stopped(None, Some(message.clone())).await;
                    return Err(anyhow!(message));
                }
                Err(_) => {
                    let _ = child.start_kill();
                    match timeout(STOP_TIMEOUT, child.wait()).await {
                        Ok(Ok(status)) => {
                            self.set_stopped(Some(format!("{exit_message}: {status}")), None).await;
                        }
                        Ok(Err(err)) => {
                            let message = format!("failed to kill mihomo process: {err}");
                            self.set_unhealthy(message.clone()).await;
                            return Err(anyhow!(message));
                        }
                        Err(_) => {
                            let message = format!("timed out stopping mihomo after {}s", STOP_TIMEOUT.as_secs());
                            self.set_unhealthy(message.clone()).await;
                            return Err(anyhow!(message));
                        }
                    }
                }
            }
            let _ = self.remove_runtime_markers().await;
        } else if let Some(pid) = self.read_pid_file().await {
            self.stop_external_pid(pid, exit_message).await?;
        } else {
            self.set_stopped(None, None).await;
        }

        Ok(())
    }

    async fn refresh_child_exit(&self) -> KernelSnapshot {
        let (snapshot, changed) = {
            let mut inner = self.inner.lock().await;
            let previous = inner.snapshot.clone();
            if let Some(child) = inner.child.as_mut() {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        inner.child = None;
                        if !matches!(inner.snapshot.state, KernelState::Stopped | KernelState::Stopping) {
                            inner.snapshot.state = KernelState::Crashed;
                            inner.snapshot.pid = None;
                            inner.snapshot.last_exit = Some(status.to_string());
                            inner.health_failures = 0;
                        }
                    }
                    Ok(None) => {
                        if matches!(inner.snapshot.state, KernelState::Starting | KernelState::Restarting) {
                            inner.snapshot.state = KernelState::Running;
                        }
                    }
                    Err(err) => {
                        inner.snapshot.state = KernelState::Unhealthy;
                        inner.snapshot.last_error = Some(err.to_string());
                    }
                }
            }

            let snapshot = inner.snapshot.clone();
            let changed = snapshot != previous;
            drop(inner);
            (snapshot, changed)
        };
        if changed {
            if matches!(snapshot.state, KernelState::Crashed) {
                let _ = self.remove_runtime_markers().await;
            }
            self.emit_kernel_state_changed(snapshot.clone());
        }
        snapshot
    }

    async fn snapshot_state(&self) -> KernelState {
        self.inner.lock().await.snapshot.state
    }

    async fn external_running_snapshot(&self) -> Option<KernelSnapshot> {
        let snapshot = self.external_snapshot().await;
        matches!(snapshot.state, KernelState::Running | KernelState::Unhealthy).then_some(snapshot)
    }

    async fn adopt_external_snapshot(&self, snapshot: KernelSnapshot) {
        let mut inner = self.inner.lock().await;
        inner.snapshot = snapshot.clone();
        inner.health_failures = if matches!(snapshot.state, KernelState::Unhealthy) {
            HEALTH_FAILURE_THRESHOLD
        } else {
            0
        };
        inner.current_job = None;
        drop(inner);
        self.emit_kernel_state_changed(snapshot);
    }

    async fn busy_status(&self) -> OperationStatus {
        let snapshot = self.snapshot().await;
        let inner = self.inner.lock().await;
        OperationStatus {
            accepted: false,
            current_job: inner.current_job.clone(),
            state: snapshot.state,
            owner: snapshot.owner,
            message: Some("Core 正忙，操作未接受".into()),
        }
    }

    async fn status(&self, accepted: bool, current_job: Option<String>) -> OperationStatus {
        let snapshot = self.snapshot().await;
        OperationStatus {
            accepted,
            current_job,
            state: snapshot.state,
            owner: snapshot.owner,
            message: None,
        }
    }

    async fn set_job_state(&self, job: &str, state: KernelState, last_error: Option<String>) {
        let mut inner = self.inner.lock().await;
        inner.current_job = Some(job.into());
        inner.snapshot.state = state;
        inner.snapshot.last_error = last_error;
        let snapshot = inner.snapshot.clone();
        drop(inner);
        self.emit_kernel_state_changed(snapshot);
    }

    async fn set_stopped_after_failure(&self, message: String) {
        self.set_stopped(None, Some(message)).await;
    }

    async fn set_stopped(&self, last_exit: Option<String>, last_error: Option<String>) {
        let version = self.local_mihomo_version().await;
        let mut inner = self.inner.lock().await;
        inner.snapshot.state = KernelState::Stopped;
        inner.snapshot.owner = KernelOwner::Stopped;
        inner.snapshot.owner_detail = None;
        inner.snapshot.pid = None;
        inner.snapshot.version = version;
        inner.snapshot.last_exit = last_exit;
        inner.snapshot.last_error = last_error;
        inner.health_failures = 0;
        inner.current_job = None;
        let snapshot = inner.snapshot.clone();
        drop(inner);
        self.emit_kernel_state_changed(snapshot);
    }

    async fn set_unhealthy(&self, message: String) {
        let mut inner = self.inner.lock().await;
        inner.snapshot.state = KernelState::Unhealthy;
        inner.snapshot.last_error = Some(message);
        inner.health_failures = HEALTH_FAILURE_THRESHOLD;
        inner.current_job = None;
        let snapshot = inner.snapshot.clone();
        drop(inner);
        self.emit_kernel_state_changed(snapshot);
    }

    async fn should_probe_health(&self) -> bool {
        let inner = self.inner.lock().await;
        inner.current_job.is_none()
            && inner.child.is_some()
            && matches!(inner.snapshot.state, KernelState::Running | KernelState::Unhealthy)
    }

    async fn record_health_success(&self, version: String) -> KernelSnapshot {
        let (snapshot, changed) = {
            let mut inner = self.inner.lock().await;
            let previous = inner.snapshot.clone();
            if inner.child.is_some() && matches!(inner.snapshot.state, KernelState::Running | KernelState::Unhealthy) {
                inner.health_failures = 0;
                inner.snapshot.state = KernelState::Running;
                inner.snapshot.version = Some(version);
                inner.snapshot.last_error = None;
            }
            let snapshot = inner.snapshot.clone();
            let changed = snapshot != previous;
            drop(inner);
            (snapshot, changed)
        };
        if changed {
            self.emit_kernel_state_changed(snapshot.clone());
        }
        snapshot
    }

    async fn stopped_snapshot(&self) -> KernelSnapshot {
        KernelSnapshot {
            version: self.local_mihomo_version().await,
            ..KernelSnapshot::stopped()
        }
    }

    async fn local_mihomo_version(&self) -> Option<String> {
        if path_requires_existing_file(&self.config.mihomo_bin) && !self.config.mihomo_bin.is_file() {
            return None;
        }

        let output = timeout(
            LOCAL_VERSION_TIMEOUT,
            Command::new(&self.config.mihomo_bin)
                .arg("-v")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output(),
        )
        .await
        .ok()?
        .ok()?;

        let mut content = String::new();
        content.push_str(&String::from_utf8_lossy(&output.stdout));
        content.push('\n');
        content.push_str(&String::from_utf8_lossy(&output.stderr));
        parse_mihomo_version_output(&content)
    }

    async fn record_health_failure(&self, message: String) -> KernelSnapshot {
        let (snapshot, changed) = {
            let mut inner = self.inner.lock().await;
            let previous = inner.snapshot.clone();
            if inner.child.is_some() && matches!(inner.snapshot.state, KernelState::Running | KernelState::Unhealthy) {
                inner.health_failures = inner.health_failures.saturating_add(1);
                inner.snapshot.last_error = Some(format!(
                    "mihomo health check failed ({}/{}): {message}",
                    inner.health_failures, HEALTH_FAILURE_THRESHOLD
                ));
                if inner.health_failures >= HEALTH_FAILURE_THRESHOLD {
                    inner.snapshot.state = KernelState::Unhealthy;
                }
            }
            let snapshot = inner.snapshot.clone();
            let changed = snapshot != previous;
            drop(inner);
            (snapshot, changed)
        };
        if changed {
            self.emit_kernel_state_changed(snapshot.clone());
        }
        snapshot
    }

    fn emit_kernel_state_changed(&self, snapshot: KernelSnapshot) {
        if let Some(events) = &self.events {
            events.emit_kernel_state_changed(snapshot);
        }
    }

    fn log_dir(&self) -> PathBuf {
        self.config.home_dir.join("logs")
    }

    fn mihomo_log_path(&self) -> PathBuf {
        self.log_dir().join(MIHOMO_LOG_FILE)
    }

    async fn read_persisted_logs(&self) -> Result<Vec<String>> {
        match tokio::fs::read_to_string(self.mihomo_log_path()).await {
            Ok(content) => Ok(tail_lines(content)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(err) => Err(err).with_context(|| format!("failed to read {}", self.mihomo_log_path().display())),
        }
    }

    async fn append_kernel_log(&self, line: String) {
        append_log(&self.inner, line).await;
    }

    async fn write_pid_file(&self, pid: Option<u32>) -> Result<()> {
        let Some(pid) = pid else {
            return Ok(());
        };
        tokio::fs::create_dir_all(&self.config.home_dir)
            .await
            .with_context(|| format!("failed to create {}", self.config.home_dir.display()))?;
        tokio::fs::write(&self.config.pid_path, pid.to_string())
            .await
            .with_context(|| format!("failed to write {}", self.config.pid_path.display()))
    }

    async fn read_pid_file(&self) -> Option<u32> {
        let content = tokio::fs::read_to_string(&self.config.pid_path).await.ok()?;
        content.trim().parse::<u32>().ok()
    }

    async fn write_owner_marker(&self, pid: u32, owner: KernelOwner, detail: Option<String>) -> Result<()> {
        tokio::fs::create_dir_all(&self.config.home_dir)
            .await
            .with_context(|| format!("failed to create {}", self.config.home_dir.display()))?;
        let marker = OwnerMarker { pid, owner, detail };
        let content = serde_json::to_vec_pretty(&marker).context("failed to serialize mihomo owner marker")?;
        tokio::fs::write(&self.config.owner_path, content)
            .await
            .with_context(|| format!("failed to write {}", self.config.owner_path.display()))
    }

    async fn read_owner_marker(&self) -> Option<OwnerMarker> {
        let content = tokio::fs::read(&self.config.owner_path).await.ok()?;
        serde_json::from_slice::<OwnerMarker>(&content).ok()
    }

    async fn remove_runtime_markers(&self) -> Result<()> {
        let pid_result = tokio::fs::remove_file(&self.config.pid_path).await;
        if let Err(err) = &pid_result
            && err.kind() != std::io::ErrorKind::NotFound
        {
            return Err(anyhow!(err.to_string()))
                .with_context(|| format!("failed to remove {}", self.config.pid_path.display()));
        }
        let owner_result = tokio::fs::remove_file(&self.config.owner_path).await;
        if let Err(err) = &owner_result
            && err.kind() != std::io::ErrorKind::NotFound
        {
            return Err(anyhow!(err.to_string()))
                .with_context(|| format!("failed to remove {}", self.config.owner_path.display()));
        }
        Ok(())
    }

    async fn owner_for_pid(&self, pid: u32) -> (KernelOwner, Option<String>) {
        if let Some(marker) = self.read_owner_marker().await
            && marker.pid == pid
        {
            return (marker.owner, marker.detail);
        }

        if let Some(service_name) = detect_systemd_service(pid).await {
            return (KernelOwner::Systemd, Some(service_name));
        }

        (KernelOwner::External, None)
    }

    async fn signal_and_wait_pid(&self, pid: u32, exit_message: &'static str) -> Result<()> {
        if !process_exists(pid) {
            let _ = self.remove_runtime_markers().await;
            self.set_stopped(None, None).await;
            return Ok(());
        }

        signal_process(pid, libc::SIGINT)?;
        for _ in 0..50 {
            if !process_exists(pid) {
                let _ = self.remove_runtime_markers().await;
                self.set_stopped(Some(format!("{exit_message}: pid {pid}")), None).await;
                return Ok(());
            }
            sleep(Duration::from_millis(100)).await;
        }

        let message = format!("timed out stopping mihomo pid {pid}");
        self.set_unhealthy(message.clone()).await;
        anyhow::bail!(message)
    }

    async fn signal_and_wait_child(&self, child: &mut Child, pid: u32, exit_message: &'static str) -> Result<()> {
        signal_process(pid, libc::SIGINT)?;
        match timeout(STOP_TIMEOUT, child.wait()).await {
            Ok(Ok(status)) => {
                let _ = self.remove_runtime_markers().await;
                self.set_stopped(Some(format!("{exit_message}: {status}")), None).await;
                Ok(())
            }
            Ok(Err(err)) => {
                let message = format!("failed to wait mihomo process: {err}");
                self.set_unhealthy(message.clone()).await;
                Err(anyhow!(message))
            }
            Err(_) => {
                let _ = child.start_kill();
                match timeout(STOP_TIMEOUT, child.wait()).await {
                    Ok(Ok(status)) => {
                        let _ = self.remove_runtime_markers().await;
                        self.set_stopped(Some(format!("{exit_message}: {status}")), None).await;
                        Ok(())
                    }
                    Ok(Err(err)) => {
                        let message = format!("failed to kill mihomo process: {err}");
                        self.set_unhealthy(message.clone()).await;
                        Err(anyhow!(message))
                    }
                    Err(_) => {
                        let message = format!("timed out stopping mihomo after {}s", STOP_TIMEOUT.as_secs());
                        self.set_unhealthy(message.clone()).await;
                        Err(anyhow!(message))
                    }
                }
            }
        }
    }

    async fn cleanup_spawned_child(&self, child: &mut Child, pid: Option<u32>, exit_message: &'static str) {
        let result = if let Some(pid) = pid {
            self.signal_and_wait_child(child, pid, exit_message).await
        } else {
            let _ = child.start_kill();
            match child.wait().await {
                Ok(status) => {
                    self.set_stopped(Some(format!("{exit_message}: {status}")), None).await;
                    Ok(())
                }
                Err(err) => Err(anyhow!(format!("failed to wait mihomo process: {err}"))),
            }
        };
        if let Err(err) = result {
            self.set_unhealthy(err.to_string()).await;
        }
    }

    async fn stop_external_pid(&self, pid: u32, exit_message: &'static str) -> Result<()> {
        #[cfg(unix)]
        {
            self.signal_and_wait_pid(pid, exit_message).await
        }

        #[cfg(not(unix))]
        {
            let _ = pid;
            let _ = exit_message;
            anyhow::bail!("stopping detached mihomo is not implemented on this platform")
        }
    }
}

const GEO_RESOURCE_FILES: &[&str] = &["Country.mmdb", "geoip.dat", "geosite.dat"];

enum ForegroundExit {
    Process(std::process::ExitStatus),
    Shutdown,
    WaitError(std::io::Error),
}

async fn wait_for_child_or_shutdown(child: &mut Child) -> ForegroundExit {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{Signal, SignalKind, signal};

        async fn recv_signal(signal: &mut Option<Signal>) {
            if let Some(signal) = signal {
                let _ = signal.recv().await;
            } else {
                std::future::pending::<()>().await;
            }
        }

        let mut sigterm = signal(SignalKind::terminate()).ok();
        let mut sigint = signal(SignalKind::interrupt()).ok();

        tokio::select! {
            result = child.wait() => match result {
                Ok(status) => ForegroundExit::Process(status),
                Err(err) => ForegroundExit::WaitError(err),
            },
            () = recv_signal(&mut sigterm) => ForegroundExit::Shutdown,
            () = recv_signal(&mut sigint) => ForegroundExit::Shutdown,
        }
    }

    #[cfg(not(unix))]
    {
        match child.wait().await {
            Ok(status) => ForegroundExit::Process(status),
            Err(err) => ForegroundExit::WaitError(err),
        }
    }
}

pub fn controller_client_config(config: &KernelProcessConfig) -> MihomoClientConfig {
    with_secret(
        MihomoClientConfig::unix(config.ipc_path.clone()),
        config.secret.as_deref(),
    )
}

fn health_client_config(config: &KernelProcessConfig) -> MihomoClientConfig {
    controller_client_config(config).with_timeout(HEALTH_CHECK_TIMEOUT)
}

fn with_secret(config: MihomoClientConfig, secret: Option<&str>) -> MihomoClientConfig {
    match secret.filter(|secret| !secret.is_empty()) {
        Some(secret) => config.with_secret(secret),
        None => config,
    }
}

fn path_requires_existing_file(path: &Path) -> bool {
    path.is_absolute() || path.components().count() > 1
}

fn process_exists(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Ok(raw_pid) = i32::try_from(pid) else {
            return false;
        };
        // SAFETY: kill with signal 0 checks process existence and does not send a signal.
        unsafe { libc::kill(raw_pid, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
    }

    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

fn signal_process(pid: u32, signal: i32) -> Result<()> {
    #[cfg(unix)]
    {
        let raw_pid = i32::try_from(pid).context("pid does not fit platform pid_t")?;
        // SAFETY: kill is called with a pid read from a local pid file and a signal selected by this process.
        let result = unsafe { libc::kill(raw_pid, signal) };
        if result == 0 {
            return Ok(());
        }

        let message = std::io::Error::last_os_error().to_string();
        anyhow::bail!("failed to signal mihomo pid {pid}: {message}");
    }

    #[cfg(not(unix))]
    {
        let _ = pid;
        let _ = signal;
        anyhow::bail!("process signaling is not implemented on this platform")
    }
}

async fn detect_systemd_service(pid: u32) -> Option<String> {
    let content = tokio::fs::read_to_string(format!("/proc/{pid}/cgroup")).await.ok()?;
    for line in content.lines() {
        let path = line.rsplit_once(':').map_or(line, |(_, path)| path);
        for segment in path.split('/') {
            if segment.ends_with(".service") {
                return Some(segment.to_owned());
            }
        }
    }
    None
}

fn parse_mihomo_version_output(content: &str) -> Option<String> {
    content
        .split_whitespace()
        .find(|token| {
            token
                .strip_prefix('v')
                .is_some_and(|rest| rest.chars().next().is_some_and(|ch| ch.is_ascii_digit()))
                || token.chars().next().is_some_and(|ch| ch.is_ascii_digit()) && token.contains('.')
        })
        .map(|token| token.trim_matches(|ch: char| matches!(ch, ',' | ';')).to_owned())
}

async fn append_log(inner: &Arc<Mutex<KernelInner>>, line: String) {
    let mut inner = inner.lock().await;
    if inner.logs.len() >= LOG_LIMIT {
        inner.logs.pop_front();
    }
    inner.logs.push_back(line);
}

fn tail_lines(content: String) -> Vec<String> {
    let mut lines = content.lines().map(str::to_owned).collect::<Vec<_>>();
    if lines.len() > LOG_LIMIT {
        lines.drain(0..lines.len() - LOG_LIMIT);
    }
    lines
}

fn merge_logs(mut persisted: Vec<String>, memory: Vec<String>) -> Vec<String> {
    persisted.extend(memory);
    if persisted.len() > LOG_LIMIT {
        persisted.drain(0..persisted.len() - LOG_LIMIT);
    }
    persisted
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::{
        fs,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use clash_core::{AppPaths, ConfigStore, IAppSettings, KernelOwner, KernelState, RuntimeConfigGenerator};
    use clash_mihomo::ControllerEndpoint;

    use super::{
        HEALTH_FAILURE_THRESHOLD, KernelManager, KernelProcessConfig, MIHOMO_LOG_FILE, OWNER_FILE,
        controller_client_config,
    };

    #[test]
    fn controller_client_config_uses_unix_socket_without_tcp_fallback() {
        let root = temp_root("kernel-controller-config");
        let paths = AppPaths::from_home(root.join("home"));
        let expected_ipc_path = paths.ipc_path.clone();
        let config = KernelProcessConfig {
            mihomo_bin: root.join("mihomo"),
            home_dir: paths.home_dir.clone(),
            resource_dir: paths.resources_dir.clone(),
            ipc_path: paths.ipc_path.clone(),
            pid_path: paths.home_dir.join("mihomo.pid"),
            owner_path: paths.home_dir.join(OWNER_FILE),
            secret: Some("secret".into()),
        };

        let client = controller_client_config(&config);

        assert_eq!(client.secret.as_deref(), Some("secret"));
        assert!(matches!(
            client.endpoint,
            ControllerEndpoint::Unix { path } if path == expected_ipc_path
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn start_stop_restart_update_kernel_state() {
        let root = temp_root("kernel");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create temp root");
        let bin = fake_mihomo_bin(&root);

        let paths = AppPaths::from_home(root.join("home"));
        let loaded = ConfigStore::new(paths.clone())
            .initialize()
            .await
            .expect("initialize config");
        let runtime = RuntimeConfigGenerator::from_loaded(&loaded);

        let manager = KernelManager::new(
            KernelProcessConfig {
                mihomo_bin: bin,
                home_dir: paths.home_dir.clone(),
                resource_dir: paths.resources_dir.clone(),
                ipc_path: paths.ipc_path.clone(),
                pid_path: paths.home_dir.join("mihomo.pid"),
                owner_path: paths.home_dir.join(OWNER_FILE),
                secret: None,
            },
            runtime,
        );

        let started = manager.start().await.expect("start fake mihomo");
        assert!(started.accepted);
        assert!(paths.runtime_config.is_file());
        let running = manager.snapshot().await;
        assert_eq!(running.state, KernelState::Running);
        assert!(running.pid.is_some());

        let duplicate = manager.start().await.expect("duplicate start");
        assert!(!duplicate.accepted);
        assert_eq!(duplicate.state, KernelState::Running);

        let restarted = manager.restart().await.expect("restart fake mihomo");
        assert!(restarted.accepted);
        assert_eq!(manager.snapshot().await.state, KernelState::Running);

        let stopped = manager.stop().await.expect("stop fake mihomo");
        assert!(stopped.accepted);
        assert_eq!(manager.snapshot().await.state, KernelState::Stopped);

        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn health_check_marks_running_process_unhealthy_after_probe_failures() {
        let root = temp_root("kernel-health");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create temp root");
        let bin = fake_mihomo_bin(&root);

        let paths = AppPaths::from_home(root.join("home"));
        let loaded = ConfigStore::new(paths.clone())
            .initialize()
            .await
            .expect("initialize config");
        let runtime = RuntimeConfigGenerator::from_loaded(&loaded);
        let missing_socket = root.join("missing.sock");

        let manager = KernelManager::new(
            KernelProcessConfig {
                mihomo_bin: bin,
                home_dir: paths.home_dir.clone(),
                resource_dir: paths.resources_dir.clone(),
                ipc_path: missing_socket,
                pid_path: paths.home_dir.join("mihomo.pid"),
                owner_path: paths.home_dir.join(OWNER_FILE),
                secret: None,
            },
            runtime,
        );

        manager.start().await.expect("start fake mihomo");
        for _ in 0..HEALTH_FAILURE_THRESHOLD {
            manager.check_health_once().await;
        }

        let snapshot = manager.snapshot().await;
        assert_eq!(snapshot.state, KernelState::Unhealthy);
        assert!(
            snapshot
                .last_error
                .as_deref()
                .is_some_and(|message| message.contains("mihomo health check failed"))
        );

        let duplicate = manager.start().await.expect("duplicate start");
        assert!(!duplicate.accepted);
        assert_eq!(duplicate.state, KernelState::Unhealthy);

        manager.stop().await.expect("stop fake mihomo");
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn external_snapshot_reports_local_version_when_core_is_stopped() {
        let root = temp_root("kernel-stopped-version");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create temp root");
        let bin = fake_mihomo_bin(&root);

        let paths = AppPaths::from_home(root.join("home"));
        let loaded = ConfigStore::new(paths.clone())
            .initialize()
            .await
            .expect("initialize config");
        let runtime = RuntimeConfigGenerator::from_loaded(&loaded);

        let manager = KernelManager::new(
            KernelProcessConfig {
                mihomo_bin: bin,
                home_dir: paths.home_dir.clone(),
                resource_dir: paths.resources_dir.clone(),
                ipc_path: paths.ipc_path.clone(),
                pid_path: paths.home_dir.join("mihomo.pid"),
                owner_path: paths.home_dir.join(OWNER_FILE),
                secret: None,
            },
            runtime,
        );

        let snapshot = manager.external_snapshot().await;

        assert_eq!(snapshot.state, KernelState::Stopped);
        assert_eq!(snapshot.version.as_deref(), Some("v9.8.7"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parse_mihomo_version_output_extracts_version_token() {
        assert_eq!(
            super::parse_mihomo_version_output("Mihomo Meta v1.19.27 linux amd64 with go1.26.4"),
            Some("v1.19.27".to_owned())
        );
        assert_eq!(
            super::parse_mihomo_version_output("version 1.19.27"),
            Some("1.19.27".to_owned())
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn start_reuses_existing_pid_file_instead_of_spawning_duplicate() {
        let root = temp_root("kernel-external-start");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create temp root");
        let bin = fake_mihomo_bin(&root);

        let paths = AppPaths::from_home(root.join("home"));
        let loaded = ConfigStore::new(paths.clone())
            .initialize()
            .await
            .expect("initialize config");
        let runtime = RuntimeConfigGenerator::from_loaded(&loaded);
        let config = KernelProcessConfig {
            mihomo_bin: bin,
            home_dir: paths.home_dir.clone(),
            resource_dir: paths.resources_dir.clone(),
            ipc_path: root.join("missing.sock"),
            pid_path: paths.home_dir.join("mihomo.pid"),
            owner_path: paths.home_dir.join(OWNER_FILE),
            secret: None,
        };

        let first = KernelManager::new(config.clone(), runtime.clone());
        let started = first.start().await.expect("start fake mihomo");
        assert!(started.accepted);
        let pid = fs::read_to_string(&config.pid_path).expect("read pid file");

        let second = KernelManager::new(config, runtime);
        let duplicate = second.start().await.expect("duplicate external start");

        assert!(!duplicate.accepted);
        assert_eq!(duplicate.state, KernelState::Unhealthy);
        assert_eq!(
            fs::read_to_string(paths.home_dir.join("mihomo.pid")).expect("read pid file after duplicate"),
            pid
        );

        first.stop().await.expect("stop fake mihomo");
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn run_foreground_writes_owner_marker_and_cleans_up() {
        let root = temp_root("kernel-run-foreground");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create temp root");
        let bin = fake_mihomo_bin(&root);

        let paths = AppPaths::from_home(root.join("home"));
        let loaded = ConfigStore::new(paths.clone())
            .initialize()
            .await
            .expect("initialize config");
        let runtime = RuntimeConfigGenerator::from_loaded(&loaded);
        let config = KernelProcessConfig {
            mihomo_bin: bin,
            home_dir: paths.home_dir.clone(),
            resource_dir: paths.resources_dir.clone(),
            ipc_path: paths.ipc_path.clone(),
            pid_path: paths.home_dir.join("mihomo.pid"),
            owner_path: paths.home_dir.join(OWNER_FILE),
            secret: None,
        };
        let pid_path = config.pid_path.clone();
        let owner_path = config.owner_path.clone();
        let manager = KernelManager::new(config, runtime);

        let handle = tokio::spawn(async move {
            manager
                .run_foreground(KernelOwner::Systemd, Some("test-clash-tui.service".into()))
                .await
        });

        for _ in 0..50 {
            if owner_path.is_file() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        let owner_marker = fs::read_to_string(&owner_path).expect("read owner marker");
        assert!(owner_marker.contains("\"owner\": \"systemd\""));
        assert!(owner_marker.contains("test-clash-tui.service"));
        let pid = fs::read_to_string(&pid_path)
            .expect("read pid file")
            .trim()
            .parse::<u32>()
            .expect("parse pid");

        super::signal_process(pid, libc::SIGINT).expect("signal fake mihomo");
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("run_foreground timeout")
            .expect("join foreground")
            .expect("run foreground");

        assert!(!pid_path.exists());
        assert!(!owner_path.exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn start_syncs_packaged_geo_resources_to_mihomo_home() {
        let root = temp_root("kernel-geo");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create temp root");
        let bin = fake_mihomo_bin(&root);

        let home_dir = root.join("home");
        let resource_dir = root.join("release-resources");
        let paths = AppPaths::new(&home_dir, &resource_dir);
        let loaded = ConfigStore::new(paths.clone())
            .initialize()
            .await
            .expect("initialize config");
        let runtime = RuntimeConfigGenerator::from_loaded(&loaded);

        fs::write(resource_dir.join("Country.mmdb"), "packaged-country").expect("write Country.mmdb");
        fs::write(resource_dir.join("geoip.dat"), "packaged-geoip").expect("write geoip.dat");
        fs::write(resource_dir.join("geosite.dat"), "packaged-geosite").expect("write geosite.dat");
        fs::write(home_dir.join("geoip.dat"), "existing-geoip").expect("write existing geoip.dat");

        let manager = KernelManager::new(
            KernelProcessConfig {
                mihomo_bin: bin,
                home_dir: paths.home_dir.clone(),
                resource_dir: paths.resources_dir.clone(),
                ipc_path: paths.ipc_path.clone(),
                pid_path: paths.home_dir.join("mihomo.pid"),
                owner_path: paths.home_dir.join(OWNER_FILE),
                secret: None,
            },
            runtime,
        );

        manager.start().await.expect("start fake mihomo");

        assert_eq!(
            fs::read_to_string(home_dir.join("Country.mmdb")).expect("read copied Country.mmdb"),
            "packaged-country"
        );
        assert_eq!(
            fs::read_to_string(home_dir.join("geosite.dat")).expect("read copied geosite.dat"),
            "packaged-geosite"
        );
        assert_eq!(
            fs::read_to_string(home_dir.join("geoip.dat")).expect("read existing geoip.dat"),
            "existing-geoip"
        );

        manager.stop().await.expect("stop fake mihomo");
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn clear_logs_removes_persisted_and_memory_logs() {
        let root = temp_root("kernel-clear-logs");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create temp root");

        let paths = AppPaths::from_home(root.join("home"));
        let loaded = ConfigStore::new(paths.clone())
            .initialize()
            .await
            .expect("initialize config");
        let runtime = RuntimeConfigGenerator::from_loaded(&loaded);
        let log_dir = paths.home_dir.join("logs");
        fs::create_dir_all(&log_dir).expect("create log dir");
        fs::write(
            log_dir.join(MIHOMO_LOG_FILE),
            "level=info msg=boot\nlevel=error msg=boom\n",
        )
        .expect("write log file");

        let manager = KernelManager::new(
            KernelProcessConfig {
                mihomo_bin: root.join("missing-mihomo"),
                home_dir: paths.home_dir.clone(),
                resource_dir: paths.resources_dir.clone(),
                ipc_path: paths.ipc_path.clone(),
                pid_path: paths.home_dir.join("mihomo.pid"),
                owner_path: paths.home_dir.join(OWNER_FILE),
                secret: None,
            },
            runtime,
        );
        manager.append_kernel_log("memory warning".into()).await;
        assert_eq!(manager.logs().await.len(), 3);

        manager.clear_logs().await.expect("clear logs");

        assert!(manager.logs().await.is_empty());
        assert_eq!(
            fs::read_to_string(log_dir.join(MIHOMO_LOG_FILE)).expect("read cleared log"),
            ""
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn disabled_core_log_discards_mihomo_stdio() {
        let root = temp_root("kernel-core-log-disabled");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create temp root");
        let bin = fake_mihomo_bin(&root);

        let home_dir = root.join("home");
        let paths = AppPaths::from_home(&home_dir);
        let store = ConfigStore::new(paths.clone());
        let loaded = store.initialize().await.expect("initialize config");
        store
            .patch_app_settings(&IAppSettings {
                enable_core_log: Some(false),
                ..IAppSettings::default()
            })
            .await
            .expect("disable core log");
        let runtime = RuntimeConfigGenerator::from_loaded(&loaded);

        let manager = KernelManager::new(
            KernelProcessConfig {
                mihomo_bin: bin,
                home_dir: paths.home_dir.clone(),
                resource_dir: paths.resources_dir.clone(),
                ipc_path: paths.ipc_path.clone(),
                pid_path: paths.home_dir.join("mihomo.pid"),
                owner_path: paths.home_dir.join(OWNER_FILE),
                secret: None,
            },
            runtime,
        );

        manager.start().await.expect("start fake mihomo");
        tokio::time::sleep(Duration::from_millis(100)).await;
        manager.stop().await.expect("stop fake mihomo");

        let log_path = paths.home_dir.join("logs").join(MIHOMO_LOG_FILE);
        assert!(
            !log_path.exists(),
            "core log disabled should not create {}",
            log_path.display()
        );
        let _ = fs::remove_dir_all(&root);
    }

    fn temp_root(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("clash-tui-{name}-{}-{nanos}", std::process::id()))
    }

    #[cfg(unix)]
    fn fake_mihomo_bin(root: &std::path::Path) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt as _;

        let bin = root.join("fake-mihomo.sh");
        fs::write(
            &bin,
            r#"#!/usr/bin/env sh
if [ "${1:-}" = "-v" ]; then
  echo "Mihomo Meta v9.8.7 linux amd64"
  exit 0
fi
trap 'exit 0' TERM INT
echo "fake mihomo started"
while true; do sleep 1; done
"#,
        )
        .expect("write fake mihomo");
        let mut permissions = fs::metadata(&bin).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&bin, permissions).expect("chmod fake mihomo");
        bin
    }
}
