use std::{
    collections::BTreeSet,
    future::Future,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context as _, Result, anyhow, bail};
use clash_core::{KernelState, ProfileCatalog};

use crate::{state::AppState, timeouts};

use super::runtime_apply::{RuntimeApplyOptions, RuntimeApplyResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileTransactionLock {
    Try,
    Wait,
}

#[derive(Debug, Clone)]
pub enum ProfileSnapshotSpec {
    ProfilesConfig,
    ImportTarget { uid: String },
    ProfileFile { uid: String },
    ProfileWithOptionFiles { uid: String },
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeCommitOptions {
    pub apply_options: RuntimeApplyOptions,
    pub start_core: bool,
}

impl RuntimeCommitOptions {
    pub const fn apply_only(apply_options: RuntimeApplyOptions) -> Self {
        Self {
            apply_options,
            start_core: false,
        }
    }

    pub const fn with_start_core(apply_options: RuntimeApplyOptions) -> Self {
        Self {
            apply_options,
            start_core: true,
        }
    }

    const fn without_start_core(self) -> Self {
        Self {
            apply_options: self.apply_options,
            start_core: false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum RuntimePolicy {
    Never,
    Always(RuntimeCommitOptions),
    IfCurrentIs { uid: String, options: RuntimeCommitOptions },
    IfCurrentWas { uid: String, options: RuntimeCommitOptions },
}

#[derive(Debug, Clone)]
pub struct ProfileTransactionSpec {
    pub failure_context: &'static str,
    pub rollback_success_message: &'static str,
    pub rollback_failed_message: &'static str,
    pub lock: ProfileTransactionLock,
    pub snapshot: ProfileSnapshotSpec,
    pub runtime: RuntimePolicy,
}

#[derive(Debug, Clone)]
pub struct ProfileTransactionRuntime {
    pub apply: RuntimeApplyResult,
    pub started_core: bool,
}

#[derive(Debug, Clone)]
pub struct ProfileTransactionOutcome<T> {
    pub output: T,
    pub profiles: ProfileCatalog,
    pub runtime: Option<ProfileTransactionRuntime>,
}

#[derive(Debug, Clone)]
struct ProfileTransactionSnapshot {
    profiles: ProfileCatalog,
    profiles_config_path: PathBuf,
    profiles_config: Option<Vec<u8>>,
    profile_files: Vec<(PathBuf, Option<Vec<u8>>)>,
}

pub async fn run_profile_transaction<T, F, Fut>(
    state: &AppState,
    spec: ProfileTransactionSpec,
    mutation: F,
) -> Result<ProfileTransactionOutcome<T>>
where
    F: FnOnce(ProfileCatalog) -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let _guard = match spec.lock {
        ProfileTransactionLock::Try => state
            .profile_switch_lock
            .try_lock()
            .map_err(|_| anyhow!("profile switch is already running"))?,
        ProfileTransactionLock::Wait => state.profile_switch_lock.lock().await,
    };

    let snapshot = ProfileTransactionSnapshot::capture(state, &spec.snapshot).await?;
    let before_profiles = snapshot.profiles.clone();
    let runtime_policy = spec.runtime.clone();

    let transaction = async {
        let output = mutation(before_profiles.clone()).await?;
        let profiles = refresh_state_profiles(state).await?;
        let runtime = apply_runtime_policy(state, &runtime_policy, &before_profiles, &profiles, true).await?;
        Ok::<_, anyhow::Error>(ProfileTransactionOutcome {
            output,
            profiles,
            runtime,
        })
    }
    .await;

    match transaction {
        Ok(outcome) => Ok(outcome),
        Err(err) => rollback_after_failure(state, &spec, &snapshot, err).await,
    }
}

async fn rollback_after_failure<T>(
    state: &AppState,
    spec: &ProfileTransactionSpec,
    snapshot: &ProfileTransactionSnapshot,
    err: anyhow::Error,
) -> Result<ProfileTransactionOutcome<T>> {
    match restore_snapshot_and_runtime(state, spec, snapshot).await {
        Ok(()) => Err(err.context(format!("{}; {}", spec.failure_context, spec.rollback_success_message))),
        Err(rollback_err) => Err(err.context(format!(
            "{}; {}: {rollback_err}",
            spec.failure_context, spec.rollback_failed_message
        ))),
    }
}

async fn restore_snapshot_and_runtime(
    state: &AppState,
    spec: &ProfileTransactionSpec,
    snapshot: &ProfileTransactionSnapshot,
) -> Result<()> {
    snapshot.restore().await?;
    let profiles = refresh_state_profiles(state).await?;
    let restore_policy = spec.runtime.without_start_core();
    let _ = apply_runtime_policy(state, &restore_policy, &snapshot.profiles, &profiles, false).await?;
    Ok(())
}

async fn refresh_state_profiles(state: &AppState) -> Result<ProfileCatalog> {
    let profiles = state.store.load_profiles().await?;
    state.config.write().await.profiles = profiles.clone();
    Ok(profiles)
}

async fn apply_runtime_policy(
    state: &AppState,
    policy: &RuntimePolicy,
    before: &ProfileCatalog,
    after: &ProfileCatalog,
    allow_start_core: bool,
) -> Result<Option<ProfileTransactionRuntime>> {
    let Some(options) = policy.commit_options(before, after) else {
        return Ok(None);
    };
    let options = if allow_start_core {
        options
    } else {
        options.without_start_core()
    };
    let apply = if options.apply_options == RuntimeApplyOptions::default() {
        super::runtime_apply::generate_validate_and_apply(state).await?
    } else {
        super::runtime_apply::generate_validate_and_apply_with_options(state, options.apply_options).await?
    };
    let started_core = if options.start_core && matches!(apply.core_state, KernelState::Stopped | KernelState::Crashed)
    {
        super::core::start(state).await?;
        let _ = super::controller::apply_saved_proxy_selections_with_retry(
            state,
            timeouts::SAVED_PROXY_SELECTION_APPLY_TIMEOUT,
        )
        .await;
        true
    } else {
        false
    };

    Ok(Some(ProfileTransactionRuntime { apply, started_core }))
}

impl RuntimePolicy {
    fn commit_options(&self, before: &ProfileCatalog, after: &ProfileCatalog) -> Option<RuntimeCommitOptions> {
        match self {
            Self::Never => None,
            Self::Always(options) => Some(*options),
            Self::IfCurrentIs { uid, options } => (after.current.as_deref() == Some(uid)).then_some(*options),
            Self::IfCurrentWas { uid, options } => (before.current.as_deref() == Some(uid)).then_some(*options),
        }
    }

    fn without_start_core(&self) -> Self {
        match self {
            Self::Never => Self::Never,
            Self::Always(options) => Self::Always(options.without_start_core()),
            Self::IfCurrentIs { uid, options } => Self::IfCurrentIs {
                uid: uid.clone(),
                options: options.without_start_core(),
            },
            Self::IfCurrentWas { uid, options } => Self::IfCurrentWas {
                uid: uid.clone(),
                options: options.without_start_core(),
            },
        }
    }
}

impl ProfileTransactionSnapshot {
    async fn capture(state: &AppState, spec: &ProfileSnapshotSpec) -> Result<Self> {
        let profiles = state.store.load_profiles().await?;
        let profiles_config_path = state.store.paths().profiles_config.clone();
        let profiles_config = read_optional_file(&profiles_config_path).await?;
        let profile_files = match spec {
            ProfileSnapshotSpec::ProfilesConfig => Vec::new(),
            ProfileSnapshotSpec::ImportTarget { uid } => {
                let file = format!("{uid}.yaml");
                let path = profile_file_path(state, &file)?;
                vec![(path.clone(), read_optional_file(&path).await?)]
            }
            ProfileSnapshotSpec::ProfileFile { uid } => capture_profile_files(state, &profiles, uid, false).await?,
            ProfileSnapshotSpec::ProfileWithOptionFiles { uid } => {
                capture_profile_files(state, &profiles, uid, true).await?
            }
        };

        Ok(Self {
            profiles,
            profiles_config_path,
            profiles_config,
            profile_files,
        })
    }

    async fn restore(&self) -> Result<()> {
        restore_optional_file(&self.profiles_config_path, self.profiles_config.as_deref()).await?;
        for (path, content) in &self.profile_files {
            restore_optional_file(path, content.as_deref()).await?;
        }
        Ok(())
    }
}

async fn capture_profile_files(
    state: &AppState,
    profiles: &ProfileCatalog,
    uid: &str,
    include_option_refs: bool,
) -> Result<Vec<(PathBuf, Option<Vec<u8>>)>> {
    let mut uids = BTreeSet::new();
    uids.insert(uid.to_owned());
    if include_option_refs
        && let Ok(item) = profiles.get_item(uid)
        && let Some(option) = item.option.as_ref()
    {
        for option_uid in [
            option.merge.as_deref(),
            option.script.as_deref(),
            option.rules.as_deref(),
            option.proxies.as_deref(),
            option.groups.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            uids.insert(option_uid.to_owned());
        }
    }

    let mut files = Vec::new();
    for snapshot_uid in uids {
        let item = match profiles.get_item(&snapshot_uid) {
            Ok(item) => item,
            Err(err) if snapshot_uid == uid => return Err(err),
            Err(_) => continue,
        };
        let Some(file) = item.file.as_deref() else {
            continue;
        };
        let path = profile_file_path(state, file)?;
        files.push((path.clone(), read_optional_file(&path).await?));
    }
    Ok(files)
}

fn profile_file_path(state: &AppState, file: &str) -> Result<PathBuf> {
    let relative = Path::new(file);
    if file.trim().is_empty() || relative.is_absolute() {
        bail!("invalid profile file path");
    }

    let mut components = relative.components();
    let Some(Component::Normal(name)) = components.next() else {
        bail!("invalid profile file path");
    };
    if components.next().is_some() {
        bail!("profile file path must not contain directories");
    }

    Ok(state.store.paths().profiles_dir.join(name))
}

async fn read_optional_file(path: &Path) -> Result<Option<Vec<u8>>> {
    match tokio::fs::read(path).await {
        Ok(content) => Ok(Some(content)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

async fn restore_optional_file(path: &Path, content: Option<&[u8]>) -> Result<()> {
    match content {
        Some(content) => {
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            tokio::fs::write(path, content)
                .await
                .with_context(|| format!("failed to restore {}", path.display()))?;
        }
        None => match tokio::fs::remove_file(path).await {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err).with_context(|| format!("failed to remove {}", path.display())),
        },
    }
    Ok(())
}
