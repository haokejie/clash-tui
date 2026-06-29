use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use clash_core::{
    IProfiles, LocalProfileImport, PrfItem, PrfOption, RemoteProfileImport,
    config::profiles::{RemoteProfileDownload, download_remote_profile, generate_remote_uid},
};
use serde::{Deserialize, Serialize};

use crate::{
    actions::profile_transaction::{
        ProfileSnapshotSpec, ProfileTransactionLock, ProfileTransactionSpec, RuntimeCommitOptions, RuntimePolicy,
        run_profile_transaction,
    },
    state::AppState,
    timeouts,
};

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

#[derive(Debug, Clone)]
struct PreparedRemoteImport {
    input: RemoteProfileImport,
    remote: RemoteProfileDownload,
    attempt: RemoteImportAttempt,
}

#[derive(Debug, Clone)]
struct ProfileSwitchMutation {
    previous: Option<String>,
}

#[derive(Debug, Clone)]
struct RemoteImportActivationMutation {
    previous: Option<String>,
    import_profiles: IProfiles,
}

#[derive(Debug, Clone)]
struct ProfileDeleteMutation {
    previous: Option<String>,
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

pub async fn import_remote_with_retry(state: &AppState, input: &RemoteProfileImport) -> Result<RemoteImportResult> {
    let prepared = download_remote_with_retry(input).await?;
    let uid = prepared
        .input
        .uid
        .clone()
        .context("remote import requires a generated uid")?;
    let attempt = prepared.attempt.clone();
    let outcome = run_profile_transaction(state, import_transaction_spec(uid), move |_before| async move {
        state
            .store
            .commit_remote_profile_import(&prepared.input, prepared.remote)
            .await
    })
    .await?;

    Ok(RemoteImportResult {
        profiles: outcome.profiles,
        attempt,
    })
}

async fn download_remote_with_retry(input: &RemoteProfileImport) -> Result<PreparedRemoteImport> {
    let base_input = remote_import_input_with_uid(input);
    let attempts = [
        ("direct", "直连", Some(false), Some(false)),
        ("clash-proxy", "Clash 代理", Some(false), Some(true)),
        ("system-proxy", "系统代理", Some(true), Some(false)),
    ];
    let mut errors = Vec::new();

    for (strategy, label, with_proxy, self_proxy) in attempts {
        let mut attempt_input = base_input.clone();
        attempt_input.option = Some(import_proxy_option(input.option.as_ref(), with_proxy, self_proxy));
        match download_remote_profile(&attempt_input).await {
            Ok(remote) => {
                return Ok(PreparedRemoteImport {
                    input: attempt_input,
                    remote,
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
    let prepared = download_remote_with_retry(input).await?;
    let requested_uid = prepared
        .input
        .uid
        .clone()
        .context("activated remote import requires a generated uid")?;
    let attempt = prepared.attempt.clone();
    let runtime_options = activation_runtime_options(start_core);
    let mutation_state = Arc::clone(&state);
    let requested_uid_for_mutation = requested_uid.clone();
    let outcome = run_profile_transaction(
        state.as_ref(),
        ProfileTransactionSpec {
            failure_context: "remote profile activation failed after import",
            rollback_success_message: "imported profile was rolled back",
            rollback_failed_message: "imported profile rollback failed",
            lock: ProfileTransactionLock::Try,
            snapshot: ProfileSnapshotSpec::ImportTarget {
                uid: requested_uid.clone(),
            },
            runtime: RuntimePolicy::Always(runtime_options),
        },
        move |before| async move {
            let previous = before.current.clone();
            let import_profiles = mutation_state
                .store
                .commit_remote_profile_import(&prepared.input, prepared.remote)
                .await?;
            let imported_uid = imported_profile_uid(&import_profiles, Some(&requested_uid_for_mutation))
                .context("imported remote profile uid was not found")?;
            mutation_state.store.switch_profile(&imported_uid).await?;
            Ok(RemoteImportActivationMutation {
                previous,
                import_profiles,
            })
        },
    )
    .await?;
    let runtime = outcome
        .runtime
        .context("remote profile activation did not apply runtime")?;
    let imported_uid = requested_uid;
    let import = RemoteImportResult {
        profiles: outcome.output.import_profiles,
        attempt,
    };
    let activation = ProfileSwitchResult {
        requested: imported_uid.clone(),
        previous: outcome.output.previous,
        profiles: outcome.profiles,
        runtime_path: runtime.apply.runtime_path,
        runtime_validated: runtime.apply.runtime_validated,
        runtime_reloaded: runtime.apply.runtime_reloaded,
        started_core: runtime.started_core,
    };

    Ok(RemoteImportActivatedResult {
        imported_uid,
        import,
        activation,
    })
}

fn remote_import_input_with_uid(input: &RemoteProfileImport) -> RemoteProfileImport {
    let mut input = input.clone();
    if input.uid.as_deref().is_none_or(|uid| uid.trim().is_empty()) {
        input.uid = Some(generate_remote_uid());
    }
    input
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
    let requested = uid.clone();
    let mutation_state = Arc::clone(&state);
    let outcome = run_profile_transaction(
        state.as_ref(),
        ProfileTransactionSpec {
            failure_context: "profile switch failed",
            rollback_success_message: "was rolled back",
            rollback_failed_message: "rollback failed",
            lock: ProfileTransactionLock::Try,
            snapshot: ProfileSnapshotSpec::ProfilesConfig,
            runtime: RuntimePolicy::Always(activation_runtime_options(start_core)),
        },
        move |before| async move {
            before
                .get_item(&uid)
                .with_context(|| format!("profile \"uid:{uid}\" not found"))?;
            let previous = before.current.clone();
            mutation_state.store.switch_profile(&uid).await?;
            Ok(ProfileSwitchMutation { previous })
        },
    )
    .await?;
    let runtime = outcome.runtime.context("profile switch did not apply runtime")?;

    Ok(ProfileSwitchResult {
        requested,
        previous: outcome.output.previous,
        profiles: outcome.profiles,
        runtime_path: runtime.apply.runtime_path,
        runtime_validated: runtime.apply.runtime_validated,
        runtime_reloaded: runtime.apply.runtime_reloaded,
        started_core: runtime.started_core,
    })
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
    let uid = uid.to_owned();
    let outcome = run_profile_transaction(
        state,
        ProfileTransactionSpec {
            failure_context: "profile delete failed",
            rollback_success_message: "was rolled back",
            rollback_failed_message: "rollback failed",
            lock: ProfileTransactionLock::Try,
            snapshot: ProfileSnapshotSpec::ProfileWithOptionFiles { uid: uid.clone() },
            runtime: RuntimePolicy::IfCurrentWas {
                uid: uid.clone(),
                options: RuntimeCommitOptions::apply_only(super::runtime_apply::RuntimeApplyOptions::default()),
            },
        },
        move |before| async move {
            before
                .get_item(&uid)
                .with_context(|| format!("profile \"uid:{uid}\" not found"))?;
            let previous = before.current.clone();
            state.store.delete_profile(&uid).await?;
            Ok(ProfileDeleteMutation { previous })
        },
    )
    .await?;
    let runtime = outcome.runtime;
    let previous = outcome.output.previous;
    let current_changed = previous != outcome.profiles.current && previous.is_some();

    Ok(ProfileDeleteResult {
        previous,
        profiles: outcome.profiles,
        current_changed,
        runtime_path: runtime.as_ref().map(|runtime| runtime.apply.runtime_path.clone()),
        runtime_validated: runtime.as_ref().is_some_and(|runtime| runtime.apply.runtime_validated),
        runtime_reloaded: runtime.as_ref().is_some_and(|runtime| runtime.apply.runtime_reloaded),
        warning: None,
    })
}

const fn import_transaction_spec(uid: String) -> ProfileTransactionSpec {
    ProfileTransactionSpec {
        failure_context: "remote profile import failed",
        rollback_success_message: "imported profile was rolled back",
        rollback_failed_message: "imported profile rollback failed",
        lock: ProfileTransactionLock::Try,
        snapshot: ProfileSnapshotSpec::ImportTarget { uid },
        runtime: RuntimePolicy::Never,
    }
}

fn activation_runtime_options(start_core: bool) -> RuntimeCommitOptions {
    let apply_options = if start_core {
        super::runtime_apply::RuntimeApplyOptions {
            controller_ready_timeout: timeouts::START_CORE_RELOAD_CONTROLLER_READY_TIMEOUT,
        }
    } else {
        super::runtime_apply::RuntimeApplyOptions::default()
    };
    if start_core {
        RuntimeCommitOptions::with_start_core(apply_options)
    } else {
        RuntimeCommitOptions::apply_only(apply_options)
    }
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
        sync::mpsc,
    };

    #[cfg(unix)]
    use tokio::net::UnixListener;

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
        install_rejecting_direct_node_mihomo(&root);
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

        assert!(
            err.to_string().contains("imported profile was rolled back"),
            "unexpected error: {err:#}"
        );
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
        install_rejecting_direct_node_mihomo(&root);
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

        assert!(
            err.to_string().contains("imported profile was rolled back"),
            "unexpected error: {err:#}"
        );
        let profiles = state.store.load_profiles().await.expect("profiles");
        assert_eq!(profiles.current, None);
        assert!(profiles.get_item("Rfirst").is_err());
        assert!(!root.join("profiles").join("Rfirst.yaml").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn import_remote_activation_failure_reloads_restored_runtime_when_core_is_active() {
        let root = short_temp_root("rollback-reload");
        let _ = fs::remove_dir_all(&root);
        install_rejecting_direct_node_mihomo(&root);
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

        let (reload_tx, mut reload_rx) = mpsc::channel(8);
        let controller = spawn_fake_controller(state.store.paths().ipc_path.clone(), reload_tx).await;
        tokio::fs::write(
            state.store.paths().home_dir.join("mihomo.pid"),
            std::process::id().to_string(),
        )
        .await
        .expect("pid file");

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

        assert!(
            err.to_string().contains("imported profile was rolled back"),
            "unexpected error: {err:#}"
        );
        let reloaded_path = tokio::time::timeout(std::time::Duration::from_secs(2), reload_rx.recv())
            .await
            .expect("rollback should reload restored runtime")
            .expect("reload path");
        assert_eq!(reloaded_path, state.store.paths().runtime_config.to_string_lossy());
        let runtime = tokio::fs::read_to_string(&state.store.paths().runtime_config)
            .await
            .expect("runtime");
        assert!(runtime.contains("PREVIOUS-NODE"));
        assert!(!runtime.contains("DIRECT-NODE"));
        controller.abort();

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

    #[cfg(unix)]
    fn short_temp_root(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        std::path::PathBuf::from(format!("/tmp/ctui-{name}-{}-{nanos}", std::process::id()))
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

    fn install_rejecting_direct_node_mihomo(root: &Path) {
        let resources = root.join("resources");
        fs::create_dir_all(&resources).expect("resources");
        let mihomo = resources.join("mihomo");
        fs::write(
            &mihomo,
            r#"#!/bin/sh
if [ "$1" = "-t" ]; then
  config=""
  while [ "$#" -gt 0 ]; do
    if [ "$1" = "-f" ]; then
      shift
      config="$1"
      break
    fi
    shift
  done
  if [ -n "$config" ] && grep -q "DIRECT-NODE" "$config"; then
    printf 'validation failed for imported runtime\n' >&2
    exit 1
  fi
  exit 0
fi
printf 'Mihomo Meta vtest\n'
"#,
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

    #[cfg(unix)]
    async fn spawn_fake_controller(
        socket_path: std::path::PathBuf,
        reload_tx: mpsc::Sender<String>,
    ) -> tokio::task::JoinHandle<()> {
        if let Some(parent) = socket_path.parent() {
            tokio::fs::create_dir_all(parent).await.expect("socket parent");
        }
        let _ = tokio::fs::remove_file(&socket_path).await;
        let listener = UnixListener::bind(&socket_path).expect("bind fake controller");
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut buffer = vec![0_u8; 4096];
                let Ok(size) = stream.read(&mut buffer).await else {
                    continue;
                };
                let request = String::from_utf8_lossy(&buffer[..size]);
                if request.starts_with("GET /version ") {
                    let body = r#"{"version":"test-controller"}"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                } else if request.starts_with("PUT /configs ") {
                    if let Some(path) = request
                        .split("\r\n\r\n")
                        .nth(1)
                        .and_then(|body| serde_json::from_str::<serde_json::Value>(body).ok())
                        .and_then(|body| body.get("path").and_then(serde_json::Value::as_str).map(str::to_owned))
                    {
                        let _ = reload_tx.try_send(path);
                    }
                    let _ = stream
                        .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                        .await;
                } else {
                    let _ = stream
                        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                        .await;
                }
                let _ = stream.shutdown().await;
            }
        })
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
