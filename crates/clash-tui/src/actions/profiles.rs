use std::{
    collections::BTreeSet,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context as _, Result, anyhow, bail};
use clash_core::{IProfiles, KernelState, LocalProfileImport, PrfItem, PrfOption, RemoteProfileImport};
use serde::{Deserialize, Serialize};
use tokio::time::timeout;

use crate::state::AppState;

const PROFILE_SWITCH_TIMEOUT: Duration = Duration::from_secs(30);
const APPLY_SAVED_SELECTION_TIMEOUT: Duration = Duration::from_secs(8);
const START_CORE_RELOAD_CONTROLLER_READY_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSwitchResult {
    pub requested: String,
    pub previous: Option<String>,
    pub profiles: IProfiles,
    pub runtime_path: String,
    pub runtime_validated: bool,
    pub runtime_reloaded: bool,
    pub started_core: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteImportResult {
    pub profiles: IProfiles,
    pub attempt: RemoteImportAttempt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteImportActivatedResult {
    pub imported_uid: String,
    pub import: RemoteImportResult,
    pub activation: ProfileSwitchResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDeleteResult {
    pub previous: Option<String>,
    pub profiles: IProfiles,
    pub current_changed: bool,
    pub runtime_path: Option<String>,
    pub runtime_validated: bool,
    pub runtime_reloaded: bool,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteImportAttempt {
    pub strategy: String,
    pub label: String,
    pub with_proxy: bool,
    pub self_proxy: bool,
}

pub async fn list(state: &AppState) -> Result<IProfiles> {
    let profiles = state.store.load_profiles().await?;
    state.config.write().await.profiles = profiles.clone();
    Ok(profiles)
}

pub async fn current(state: &AppState) -> Result<Option<PrfItem>> {
    let profiles = list(state).await?;
    let Some(current) = profiles.current.as_deref() else {
        return Ok(None);
    };
    Ok(profiles.get_item(current).ok().cloned())
}

pub async fn import_local(state: &AppState, input: &LocalProfileImport) -> Result<IProfiles> {
    let profiles = state.store.import_local_profile(input).await?;
    state.config.write().await.profiles = profiles.clone();
    Ok(profiles)
}

pub async fn import_remote(state: &AppState, input: &RemoteProfileImport) -> Result<IProfiles> {
    let profiles = state.store.import_remote_profile(input).await?;
    state.config.write().await.profiles = profiles.clone();
    Ok(profiles)
}

pub async fn import_remote_with_retry(state: &AppState, input: &RemoteProfileImport) -> Result<RemoteImportResult> {
    let attempts = [
        ("direct", "直连", Some(false), Some(false)),
        ("clash-proxy", "Clash 代理", Some(false), Some(true)),
        ("system-proxy", "系统代理", Some(true), Some(false)),
    ];
    let mut errors = Vec::new();

    for (strategy, label, with_proxy, self_proxy) in attempts {
        let mut attempt_input = input.clone();
        attempt_input.option = Some(import_proxy_option(input.option.as_ref(), with_proxy, self_proxy));
        match import_remote(state, &attempt_input).await {
            Ok(profiles) => {
                return Ok(RemoteImportResult {
                    profiles,
                    attempt: RemoteImportAttempt {
                        strategy: strategy.into(),
                        label: label.into(),
                        with_proxy: with_proxy.unwrap_or(false),
                        self_proxy: self_proxy.unwrap_or(false),
                    },
                });
            }
            Err(err) => errors.push(format!("{label}: {}", redact_urls(&err.to_string()))),
        }
    }

    bail!("远程订阅导入全部策略失败：{}", errors.join("；"))
}

pub async fn import_remote_with_retry_and_activate(
    state: Arc<AppState>,
    input: &RemoteProfileImport,
    start_core: bool,
) -> Result<RemoteImportActivatedResult> {
    let requested_uid = input
        .uid
        .as_deref()
        .filter(|uid| !uid.trim().is_empty())
        .context("activated remote import requires a generated uid")?;
    let rollback = ProfileStoreBackup::capture_import_target(&state, requested_uid).await?;
    let import = import_remote_with_retry(&state, input).await?;
    let imported_uid = imported_profile_uid(&import.profiles, Some(requested_uid))
        .context("imported remote profile uid was not found")?;

    match activate(Arc::clone(&state), imported_uid.clone(), start_core).await {
        Ok(activation) => Ok(RemoteImportActivatedResult {
            imported_uid,
            import,
            activation,
        }),
        Err(err) => {
            let rollback_message = match restore_profile_store_backup(&state, &rollback).await {
                Ok(()) => "imported profile was rolled back".to_owned(),
                Err(rollback_err) => format!("imported profile rollback failed: {rollback_err}"),
            };
            Err(err.context(format!(
                "remote profile activation failed after import; {rollback_message}"
            )))
        }
    }
}

fn import_proxy_option(base: Option<&PrfOption>, with_proxy: Option<bool>, self_proxy: Option<bool>) -> PrfOption {
    let mut option = base.cloned().unwrap_or_default();
    option.with_proxy = with_proxy;
    option.self_proxy = self_proxy;
    option
}

fn redact_urls(message: &str) -> String {
    let mut output = Vec::new();
    for part in message.split_whitespace() {
        if part.starts_with("http://") || part.starts_with("https://") {
            output.push("[订阅链接]");
        } else {
            output.push(part);
        }
    }
    if output.is_empty() {
        "未知错误".into()
    } else {
        output.join(" ")
    }
}

pub async fn switch(state: Arc<AppState>, uid: String) -> Result<ProfileSwitchResult> {
    activate(state, uid, false).await
}

pub async fn activate(state: Arc<AppState>, uid: String, start_core: bool) -> Result<ProfileSwitchResult> {
    let Ok(_guard) = state.profile_switch_lock.try_lock() else {
        bail!("profile switch is already running");
    };

    let before = state.store.load_profiles().await?;
    let previous = before.current.clone();
    before
        .get_item(&uid)
        .with_context(|| format!("profile \"uid:{uid}\" not found"))?;

    let result = timeout(PROFILE_SWITCH_TIMEOUT, async {
        let profiles = state.store.switch_profile(&uid).await?;
        state.config.write().await.profiles = profiles.clone();

        let runtime_options = if start_core {
            super::runtime_apply::RuntimeApplyOptions {
                controller_ready_timeout: START_CORE_RELOAD_CONTROLLER_READY_TIMEOUT,
            }
        } else {
            super::runtime_apply::RuntimeApplyOptions::default()
        };
        let runtime_apply =
            super::runtime_apply::generate_validate_and_apply_with_options(&state, runtime_options).await?;
        let started_core = if start_core
            && matches!(runtime_apply.core_state, KernelState::Stopped | KernelState::Crashed)
        {
            super::core::start(state.as_ref()).await?;
            let _ =
                super::controller::apply_saved_proxy_selections_with_retry(&state, APPLY_SAVED_SELECTION_TIMEOUT).await;
            true
        } else {
            false
        };

        Ok::<_, anyhow::Error>(ProfileSwitchResult {
            requested: uid.clone(),
            previous: previous.clone(),
            profiles,
            runtime_path: runtime_apply.runtime_path,
            runtime_validated: runtime_apply.runtime_validated,
            runtime_reloaded: runtime_apply.runtime_reloaded,
            started_core,
        })
    })
    .await;

    match result {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(err)) => match rollback_profile(&state, previous.as_deref()).await {
            Ok(()) => Err(err.context("profile switch failed and was rolled back")),
            Err(rollback_err) => Err(err.context(format!("profile switch failed and rollback failed: {rollback_err}"))),
        },
        Err(_) => {
            let timeout_err = anyhow!(
                "profile switch timed out after {}s and was rolled back",
                PROFILE_SWITCH_TIMEOUT.as_secs()
            );
            match rollback_profile(&state, previous.as_deref()).await {
                Ok(()) => Err(timeout_err),
                Err(rollback_err) => Err(timeout_err.context(format!("rollback failed: {rollback_err}"))),
            }
        }
    }
}

#[must_use]
pub fn imported_profile_uid(profiles: &IProfiles, requested_uid: Option<&str>) -> Option<String> {
    let items = profiles.items.as_deref().unwrap_or_default();
    requested_uid
        .and_then(|uid| items.iter().find(|item| item.uid.as_deref() == Some(uid)))
        .and_then(|item| item.uid.clone())
        .or_else(|| {
            items
                .iter()
                .rev()
                .find(|item| item.itype.as_deref() == Some("remote"))
                .and_then(|item| item.uid.clone())
        })
}

pub async fn delete(state: &AppState, uid: &str) -> Result<ProfileDeleteResult> {
    let Ok(_guard) = state.profile_switch_lock.try_lock() else {
        bail!("profile switch is already running");
    };

    let before = state.store.load_profiles().await?;
    let previous = before.current.clone();
    before
        .get_item(uid)
        .with_context(|| format!("profile \"uid:{uid}\" not found"))?;
    let deleting_current = previous.as_deref() == Some(uid);
    let backup = if deleting_current {
        Some(ProfileStoreBackup::capture(state, &before, uid).await?)
    } else {
        None
    };
    let next_uid = deleting_current
        .then(|| next_profile_uid_after_delete(&before, uid))
        .flatten();
    let mut runtime_path = None;
    let mut runtime_validated = false;
    let mut runtime_reloaded = false;
    let warning = None;

    let profiles = if let Some(next_uid) = next_uid {
        let switched_profiles = state.store.switch_profile(&next_uid).await?;
        state.config.write().await.profiles = switched_profiles;
        match super::runtime_apply::generate_validate_and_apply(state).await {
            Ok(apply) => {
                runtime_path = Some(apply.runtime_path);
                runtime_validated = apply.runtime_validated;
                runtime_reloaded = apply.runtime_reloaded;
            }
            Err(err) => {
                rollback_profile(state, previous.as_deref()).await?;
                return Err(err.context("profile delete failed and was rolled back"));
            }
        }
        let profiles = state.store.delete_profile(uid).await?;
        state.config.write().await.profiles = profiles.clone();
        profiles
    } else {
        let profiles = state.store.delete_profile(uid).await?;
        state.config.write().await.profiles = profiles.clone();
        if deleting_current {
            match super::runtime_apply::generate_validate_and_apply(state).await {
                Ok(apply) => {
                    runtime_path = Some(apply.runtime_path);
                    runtime_validated = apply.runtime_validated;
                    runtime_reloaded = apply.runtime_reloaded;
                }
                Err(err) => {
                    if let Some(backup) = backup.as_ref() {
                        restore_delete_backup(state, backup).await?;
                    }
                    return Err(err.context("profile delete failed and was rolled back"));
                }
            }
        }
        profiles
    };
    let current_changed = deleting_current && previous != profiles.current;

    Ok(ProfileDeleteResult {
        previous,
        profiles,
        current_changed,
        runtime_path,
        runtime_validated,
        runtime_reloaded,
        warning,
    })
}

async fn rollback_profile(state: &AppState, previous: Option<&str>) -> Result<()> {
    if let Some(previous) = previous {
        let profiles = state.store.switch_profile(previous).await?;
        state.config.write().await.profiles = profiles;
        state.runtime.generate().await?;
    }
    Ok(())
}

async fn restore_delete_backup(state: &AppState, backup: &ProfileStoreBackup) -> Result<()> {
    restore_profile_store_backup(state, backup).await
}

async fn restore_profile_store_backup(state: &AppState, backup: &ProfileStoreBackup) -> Result<()> {
    backup.restore().await?;
    let profiles = state.store.load_profiles().await?;
    state.config.write().await.profiles = profiles;
    state.runtime.generate().await?;
    Ok(())
}

fn next_profile_uid_after_delete(profiles: &IProfiles, uid: &str) -> Option<String> {
    profiles
        .items
        .as_deref()
        .unwrap_or_default()
        .iter()
        .find(|item| item.uid.as_deref() != Some(uid) && matches!(item.itype.as_deref(), Some("remote" | "local")))
        .and_then(|item| item.uid.clone())
}

#[derive(Debug, Clone)]
struct ProfileStoreBackup {
    profiles_config_path: PathBuf,
    profiles_config: Option<Vec<u8>>,
    profile_files: Vec<(PathBuf, Option<Vec<u8>>)>,
}

impl ProfileStoreBackup {
    async fn capture_import_target(state: &AppState, uid: &str) -> Result<Self> {
        let profile_file = format!("{uid}.yaml");
        let profile_file_path = profile_file_path(state, &profile_file)?;
        let profiles_config_path = state.store.paths().profiles_config.clone();
        Ok(Self {
            profiles_config: read_optional_file(&profiles_config_path).await?,
            profiles_config_path,
            profile_files: vec![(profile_file_path.clone(), read_optional_file(&profile_file_path).await?)],
        })
    }

    async fn capture(state: &AppState, profiles: &IProfiles, uid: &str) -> Result<Self> {
        let mut uids = BTreeSet::new();
        uids.insert(uid.to_owned());
        if let Ok(item) = profiles.get_item(uid)
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

        let mut profile_files = Vec::new();
        for backup_uid in uids {
            let Ok(item) = profiles.get_item(&backup_uid) else {
                continue;
            };
            let Some(file) = item.file.as_deref() else {
                continue;
            };
            let path = profile_file_path(state, file)?;
            profile_files.push((path.clone(), read_optional_file(&path).await?));
        }

        let profiles_config_path = state.store.paths().profiles_config.clone();
        Ok(Self {
            profiles_config: read_optional_file(&profiles_config_path).await?,
            profiles_config_path,
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

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::{
        fs,
        path::Path,
        sync::Arc,
        time::{SystemTime, UNIX_EPOCH},
    };

    use clash_core::{IProfiles, LocalProfileImport, PrfItem, PrfOption, RemoteProfileImport};
    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::TcpListener,
    };

    use crate::{options::ClashTuiOptions, state::AppState};

    #[test]
    fn import_proxy_option_sets_retry_strategy_without_losing_base_options() {
        let base = PrfOption {
            user_agent: Some("test-agent".into()),
            update_interval: Some(120),
            timeout_seconds: Some(45),
            ..PrfOption::default()
        };

        let option = super::import_proxy_option(Some(&base), Some(false), Some(true));

        assert_eq!(option.user_agent.as_deref(), Some("test-agent"));
        assert_eq!(option.update_interval, Some(120));
        assert_eq!(option.timeout_seconds, Some(45));
        assert_eq!(option.with_proxy, Some(false));
        assert_eq!(option.self_proxy, Some(true));
    }

    #[test]
    fn import_retry_error_redacts_subscription_urls() {
        let redacted = super::redact_urls("failed to fetch https://example.invalid/sub?token=secret via direct");

        assert_eq!(redacted, "failed to fetch [订阅链接] via direct");
    }

    #[tokio::test]
    async fn switch_rolls_back_when_runtime_generation_fails() {
        let root = temp_root("switch-rollback");
        let _ = fs::remove_dir_all(&root);
        let options =
            ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options");
        let state = AppState::initialize(options).await.expect("state");

        state
            .store
            .import_local_profile(&LocalProfileImport {
                uid: Some("Lgood".into()),
                name: Some("Good".into()),
                file_data: "proxies: []\nproxy-groups: []\nrules: []\n".into(),
            })
            .await
            .expect("good profile");
        state
            .store
            .import_local_profile(&LocalProfileImport {
                uid: Some("Lbad".into()),
                name: Some("Bad".into()),
                file_data: "not: [valid\n".into(),
            })
            .await
            .expect_err("invalid import");

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn activate_switches_current_and_generates_runtime_without_starting_core() {
        let root = temp_root("activate");
        let _ = fs::remove_dir_all(&root);
        install_fake_mihomo(&root);
        let options =
            ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options");
        let state = AppState::initialize(options).await.expect("state");

        state
            .store
            .import_local_profile(&LocalProfileImport {
                uid: Some("Lone".into()),
                name: Some("One".into()),
                file_data: "proxies: []\nproxy-groups: []\nrules: []\n".into(),
            })
            .await
            .expect("first profile");
        state
            .store
            .import_local_profile(&LocalProfileImport {
                uid: Some("Ltwo".into()),
                name: Some("Two".into()),
                file_data: "proxies: []\nproxy-groups: []\nrules: []\n".into(),
            })
            .await
            .expect("second profile");

        let result = super::activate(Arc::clone(&state), "Ltwo".into(), false)
            .await
            .expect("activate");

        assert_eq!(result.profiles.current.as_deref(), Some("Ltwo"));
        assert_eq!(result.previous.as_deref(), Some("Lone"));
        assert!(!result.started_core);
        assert!(result.runtime_validated);
        assert!(!result.runtime_reloaded);
        assert!(std::path::Path::new(&result.runtime_path).is_file());

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn import_remote_activation_failure_restores_previous_current_and_removes_import() {
        let root = temp_root("import-activate-rollback-previous");
        let _ = fs::remove_dir_all(&root);
        install_failing_mihomo(&root);
        let options =
            ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options");
        let state = AppState::initialize(options).await.expect("state");
        state
            .store
            .import_local_profile(&LocalProfileImport {
                uid: Some("Lone".into()),
                name: Some("One".into()),
                file_data: "proxies:\n  - name: PREVIOUS-NODE\n    type: direct\nproxy-groups:\n  - name: Previous\n    type: select\n    proxies:\n      - PREVIOUS-NODE\nrules:\n  - MATCH,Previous\n".into(),
            })
            .await
            .expect("previous profile");

        let url = serve_remote_profile(valid_remote_profile()).await;
        let err = super::import_remote_with_retry_and_activate(
            Arc::clone(&state),
            &RemoteProfileImport {
                url,
                uid: Some("Rtxn".into()),
                name: None,
                desc: None,
                option: None,
            },
            false,
        )
        .await
        .expect_err("activation should fail");

        assert!(err.to_string().contains("imported profile was rolled back"));
        let profiles = state.store.load_profiles().await.expect("profiles");
        assert_eq!(profiles.current.as_deref(), Some("Lone"));
        assert!(profiles.get_item("Rtxn").is_err());
        assert!(!root.join("profiles").join("Rtxn.yaml").exists());
        let runtime = tokio::fs::read_to_string(root.join("mihomo-runtime.yaml"))
            .await
            .expect("runtime");
        assert!(runtime.contains("PREVIOUS-NODE"));
        assert!(!runtime.contains("DIRECT-NODE"));

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn first_import_remote_activation_failure_restores_empty_current() {
        let root = temp_root("import-activate-rollback-empty");
        let _ = fs::remove_dir_all(&root);
        install_failing_mihomo(&root);
        let options =
            ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options");
        let state = AppState::initialize(options).await.expect("state");

        let url = serve_remote_profile(valid_remote_profile()).await;
        let err = super::import_remote_with_retry_and_activate(
            Arc::clone(&state),
            &RemoteProfileImport {
                url,
                uid: Some("Rfirst".into()),
                name: None,
                desc: None,
                option: None,
            },
            false,
        )
        .await
        .expect_err("activation should fail");

        assert!(err.to_string().contains("imported profile was rolled back"));
        let profiles = state.store.load_profiles().await.expect("profiles");
        assert_eq!(profiles.current, None);
        assert!(profiles.get_item("Rfirst").is_err());
        assert!(!root.join("profiles").join("Rfirst.yaml").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn delete_current_profile_selects_next_and_regenerates_runtime() {
        let root = temp_root("delete-current-runtime");
        let _ = fs::remove_dir_all(&root);
        install_fake_mihomo(&root);
        let options =
            ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options");
        let state = AppState::initialize(options).await.expect("state");

        state
            .store
            .import_local_profile(&LocalProfileImport {
                uid: Some("Lone".into()),
                name: Some("One".into()),
                file_data: "mode: global\nproxies: []\nproxy-groups: []\nrules: []\n".into(),
            })
            .await
            .expect("first profile");
        state
            .store
            .import_local_profile(&LocalProfileImport {
                uid: Some("Ltwo".into()),
                name: Some("Two".into()),
                file_data: "mode: rule\nproxies: []\nproxy-groups: []\nrules: []\n".into(),
            })
            .await
            .expect("second profile");

        let result = super::delete(&state, "Lone").await.expect("delete current");

        assert_eq!(result.previous.as_deref(), Some("Lone"));
        assert_eq!(result.profiles.current.as_deref(), Some("Ltwo"));
        assert!(result.current_changed);
        assert!(
            result
                .runtime_path
                .as_deref()
                .is_some_and(|path| std::path::Path::new(path).is_file())
        );
        assert!(result.runtime_validated);
        assert!(!result.runtime_reloaded);
        assert!(result.warning.is_none());
        let runtime = tokio::fs::read_to_string(result.runtime_path.expect("runtime path"))
            .await
            .expect("runtime");
        assert!(runtime.contains("mode: rule"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn imported_profile_uid_prefers_requested_or_last_remote() {
        let profiles = IProfiles {
            current: Some("Rold".into()),
            items: Some(vec![
                PrfItem {
                    uid: Some("Rold".into()),
                    itype: Some("remote".into()),
                    ..PrfItem::default()
                },
                PrfItem {
                    uid: Some("Rnew".into()),
                    itype: Some("remote".into()),
                    ..PrfItem::default()
                },
            ]),
        };

        assert_eq!(
            super::imported_profile_uid(&profiles, Some("Rold")).as_deref(),
            Some("Rold")
        );
        assert_eq!(super::imported_profile_uid(&profiles, None).as_deref(), Some("Rnew"));
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

    fn install_failing_mihomo(root: &Path) {
        let resources = root.join("resources");
        fs::create_dir_all(&resources).expect("resources");
        let mihomo = resources.join("mihomo");
        fs::write(
            &mihomo,
            "#!/bin/sh\nif [ \"$1\" = \"-t\" ]; then printf 'validation failed\\n' >&2; exit 1; fi\nprintf 'Mihomo Meta vtest\\n'\n",
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

    fn valid_remote_profile() -> &'static str {
        "proxies:\n  - name: DIRECT-NODE\n    type: direct\nproxy-groups:\n  - name: Proxy\n    type: select\n    proxies:\n      - DIRECT-NODE\nrules:\n  - MATCH,Proxy\n"
    }

    async fn serve_remote_profile(body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut buffer = vec![0_u8; 2048];
            let _ = stream.read(&mut buffer).await.expect("read");
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/yaml\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.expect("write response");
        });
        format!("http://{addr}/sub.yaml")
    }
}
