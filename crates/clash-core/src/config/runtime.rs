use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_yaml_ng::{Mapping, Value};

use crate::{
    AppPaths, ConfigLoadResult,
    config::{
        IAppSettings, IClashTemp, IProfiles, dns,
        enhance::{use_merge, use_script},
    },
    constants::network,
    yaml,
};

const PATCH_CONFIG_INNER: [&str; 5] = ["allow-lan", "ipv6", "log-level", "unified-delay", "tunnels"];

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct IRuntime {
    pub config: Option<Mapping>,
    pub exists_keys: HashSet<String>,
    pub chain_logs: HashMap<String, Vec<(String, String)>>,
}

impl IRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn patch_config(&mut self, patch: &Mapping) {
        let Some(config) = self.config.as_mut() else {
            return;
        };

        for key in PATCH_CONFIG_INNER {
            if let Some(value) = patch.get(key) {
                config.insert(key.into(), value.clone());
            }
        }

        if let Some(Value::Mapping(patch_tun)) = patch.get("tun") {
            let mut tun = config
                .get("tun")
                .and_then(Value::as_mapping)
                .cloned()
                .unwrap_or_default();

            for (key, value) in patch_tun {
                if let Some(key) = key.as_str() {
                    tun.insert(key.to_ascii_lowercase().into(), value.clone());
                }
            }

            config.insert("tun".into(), Value::from(tun));
        }
    }

    pub fn update_proxy_chain_config(&mut self, proxy_chain_config: Option<Value>) {
        let Some(config) = self.config.as_mut() else {
            return;
        };

        if let Some(Value::Sequence(proxies)) = config.get_mut("proxies") {
            for proxy in proxies {
                if let Some(proxy) = proxy.as_mapping_mut() {
                    proxy.remove("dialer-proxy");
                }
            }
        }

        if let Some(Value::Sequence(dialer_proxies)) = proxy_chain_config
            && let Some(Value::Sequence(proxies)) = config.get_mut("proxies")
        {
            for (index, dialer_proxy) in dialer_proxies.iter().enumerate() {
                if index == 0 {
                    continue;
                }

                if let Some(Value::Mapping(proxy)) =
                    proxies.iter_mut().find(|proxy| proxy.get("name") == Some(dialer_proxy))
                    && let Some(previous) = dialer_proxies.get(index - 1)
                {
                    proxy.insert("dialer-proxy".into(), previous.clone());
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeConfigGenerator {
    paths: AppPaths,
    proxy_chain_config: Arc<RwLock<Option<Vec<String>>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeConfigResult {
    pub path: String,
    pub source_profile: Option<String>,
    pub fallback: bool,
}

impl RuntimeConfigGenerator {
    #[must_use]
    pub fn from_loaded(config: &ConfigLoadResult) -> Self {
        Self::from_paths(config.paths.clone())
    }

    #[must_use]
    pub fn from_paths(paths: AppPaths) -> Self {
        Self {
            paths,
            proxy_chain_config: Arc::new(RwLock::new(None)),
        }
    }

    pub fn set_proxy_chain_config(&self, proxy_chain_config: Option<Vec<String>>) -> Result<()> {
        let normalized = proxy_chain_config
            .map(|items| {
                items
                    .into_iter()
                    .map(|item| item.trim().to_owned())
                    .filter(|item| !item.is_empty())
                    .collect::<Vec<_>>()
            })
            .filter(|items| !items.is_empty());
        {
            let mut guard = self
                .proxy_chain_config
                .write()
                .map_err(|_| anyhow!("runtime proxy chain config lock poisoned"))?;
            *guard = normalized;
        }
        Ok(())
    }

    pub async fn read_proxy_chain_yaml(&self, proxy_chain_exit_node: &str) -> Result<String> {
        if !tokio::fs::try_exists(&self.paths.runtime_config).await.unwrap_or(false) {
            self.generate().await?;
        }
        let config = yaml::read_mapping(&self.paths.runtime_config)
            .await
            .with_context(|| format!("failed to load {}", self.paths.runtime_config.display()))?;
        proxy_chain_yaml_from_mapping(&config, proxy_chain_exit_node)
    }

    pub async fn generate(&self) -> Result<RuntimeConfigResult> {
        self.generate_with_dns_override(None).await
    }

    pub async fn generate_with_dns_override(&self, enable_dns_settings: Option<bool>) -> Result<RuntimeConfigResult> {
        let clash = self.load_clash().await?;
        let app_settings = self.load_app_settings().await?;
        let profiles = self.load_profiles().await?;
        let (mut config, used_profile) = self.current_profile_mapping(&profiles).await?;
        if used_profile {
            config = self.apply_profile_enhancements(config, &profiles).await?;
        }
        merge_default_config(&mut config, &clash.0);
        if enable_dns_settings.unwrap_or_else(|| app_settings.enable_dns_settings.unwrap_or(false)) {
            self.merge_dns_config(&mut config).await?;
        }
        apply_app_settings_overrides(&mut config, &app_settings);
        apply_controller_boundary(&mut config, &self.paths, &app_settings);
        self.apply_proxy_chain_config(&mut config)?;
        ensure_selectable_proxy_group(&mut config);

        yaml::save_yaml(&self.paths.runtime_config, &config, Some("# Generated by Clash TUI")).await?;

        Ok(RuntimeConfigResult {
            path: display_path(&self.paths.runtime_config),
            source_profile: used_profile.then(|| profiles.current.clone()).flatten(),
            fallback: !used_profile,
        })
    }

    pub async fn read_runtime_yaml(&self) -> Result<String> {
        tokio::fs::read_to_string(&self.paths.runtime_config)
            .await
            .with_context(|| format!("failed to read {}", self.paths.runtime_config.display()))
    }

    async fn load_clash(&self) -> Result<IClashTemp> {
        let mapping = yaml::read_mapping(&self.paths.clash_config)
            .await
            .with_context(|| format!("failed to load {}", self.paths.clash_config.display()))?;
        Ok(IClashTemp::from_mapping(mapping, Some(&self.paths.ipc_path)))
    }

    async fn load_app_settings(&self) -> Result<IAppSettings> {
        yaml::read_yaml(&self.paths.settings_config)
            .await
            .with_context(|| format!("failed to load {}", self.paths.settings_config.display()))
    }

    async fn load_profiles(&self) -> Result<IProfiles> {
        yaml::read_yaml(&self.paths.profiles_config)
            .await
            .with_context(|| format!("failed to load {}", self.paths.profiles_config.display()))
    }

    async fn merge_dns_config(&self, config: &mut Mapping) -> Result<()> {
        if !tokio::fs::try_exists(&self.paths.dns_config).await.unwrap_or(false) {
            return Ok(());
        }

        let dns = yaml::read_mapping(&self.paths.dns_config)
            .await
            .with_context(|| format!("failed to load {}", self.paths.dns_config.display()))?;
        dns::apply_dns_config_to_runtime(config, &dns);
        Ok(())
    }

    async fn current_profile_mapping(&self, profiles: &IProfiles) -> Result<(Mapping, bool)> {
        let Some(uid) = profiles.current.as_deref() else {
            return Ok((Mapping::new(), false));
        };

        let item = profiles.get_item(uid)?;
        let Some(file) = item.file.as_deref() else {
            return Ok((Mapping::new(), false));
        };

        let path = profile_file_path(&self.paths.profiles_dir, file);
        if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok((Mapping::new(), false));
        }

        let mapping = yaml::read_mapping(&path)
            .await
            .with_context(|| format!("failed to read current profile {}", path.display()))?;
        Ok((mapping, true))
    }

    async fn apply_profile_enhancements(&self, mut config: Mapping, profiles: &IProfiles) -> Result<Mapping> {
        let Some(current_uid) = profiles.current.as_deref() else {
            return Ok(config);
        };
        let current_item = profiles.get_item(current_uid)?;
        let profile_name = current_item.name.clone().unwrap_or_default();

        if let Some(merge) = self.load_merge_item(profiles, "Merge").await? {
            config = use_merge(&merge, config);
        }

        if let Some(script) = self.load_script_item(profiles, "Script").await? {
            let (next, _) = use_script(script, config, profile_name.clone()).await?;
            config = next;
        }

        if let Some(merge_uid) = current_item
            .option
            .as_ref()
            .and_then(|option| option.merge.as_deref())
            .filter(|uid| *uid != "Merge")
            && let Some(merge) = self.load_merge_item(profiles, merge_uid).await?
        {
            config = use_merge(&merge, config);
        }

        if let Some(script_uid) = current_item
            .option
            .as_ref()
            .and_then(|option| option.script.as_deref())
            .filter(|uid| *uid != "Script")
            && let Some(script) = self.load_script_item(profiles, script_uid).await?
        {
            let (next, _) = use_script(script, config, profile_name).await?;
            config = next;
        }

        Ok(config)
    }

    async fn load_merge_item(&self, profiles: &IProfiles, uid: &str) -> Result<Option<Mapping>> {
        let Some(path) = self.profile_item_path(profiles, uid).await? else {
            return Ok(None);
        };
        yaml::read_mapping(&path)
            .await
            .with_context(|| format!("failed to read merge profile {}", path.display()))
            .map(Some)
    }

    async fn load_script_item(&self, profiles: &IProfiles, uid: &str) -> Result<Option<String>> {
        let Some(path) = self.profile_item_path(profiles, uid).await? else {
            return Ok(None);
        };
        tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("failed to read script profile {}", path.display()))
            .map(Some)
    }

    async fn profile_item_path(&self, profiles: &IProfiles, uid: &str) -> Result<Option<PathBuf>> {
        let Ok(item) = profiles.get_item(uid) else {
            return Ok(None);
        };
        let Some(file) = item.file.as_deref() else {
            return Ok(None);
        };
        let path = profile_file_path(&self.paths.profiles_dir, file);
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            Ok(Some(path))
        } else {
            Ok(None)
        }
    }

    fn proxy_chain_config(&self) -> Result<Option<Vec<String>>> {
        self.proxy_chain_config
            .read()
            .map_err(|_| anyhow!("runtime proxy chain config lock poisoned"))
            .map(|guard| guard.clone())
    }

    fn apply_proxy_chain_config(&self, config: &mut Mapping) -> Result<()> {
        let Some(proxy_chain_config) = self.proxy_chain_config()? else {
            return Ok(());
        };
        let mut runtime = IRuntime {
            config: Some(config.clone()),
            ..IRuntime::default()
        };
        runtime.update_proxy_chain_config(Some(Value::Sequence(
            proxy_chain_config.into_iter().map(Value::String).collect(),
        )));
        if let Some(updated) = runtime.config {
            *config = updated;
        }
        Ok(())
    }
}

fn proxy_chain_yaml_from_mapping(config: &Mapping, proxy_chain_exit_node: &str) -> Result<String> {
    let Some(Value::Sequence(proxies)) = config.get(yaml_key("proxies")) else {
        bail!("failed to get proxies");
    };
    let mut proxy_name = Some(proxy_chain_exit_node.to_owned());
    let mut proxies_chain = Vec::new();
    let mut seen = HashSet::new();

    while let Some(name) = proxy_name.clone() {
        if !seen.insert(name.clone()) {
            bail!("proxy chain contains a cycle at {name}");
        }
        let Some(proxy) = proxies.iter().find(|proxy| {
            let Some(proxy_map) = proxy.as_mapping() else {
                return false;
            };
            proxy_map.get(yaml_key("name")).and_then(Value::as_str) == Some(name.as_str())
                && proxy_map.get(yaml_key("dialer-proxy")).is_some()
        }) else {
            break;
        };
        proxies_chain.push(proxy.clone());
        proxy_name = proxy
            .as_mapping()
            .and_then(|proxy| proxy.get(yaml_key("dialer-proxy")))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
    }

    if let Some(entry_name) = proxy_name
        && let Some(entry_proxy) = proxies.iter().find(|proxy| {
            proxy
                .as_mapping()
                .and_then(|proxy| proxy.get(yaml_key("name")))
                .and_then(Value::as_str)
                == Some(entry_name.as_str())
        })
        && !proxies_chain.is_empty()
    {
        proxies_chain.push(entry_proxy.clone());
    }

    proxies_chain.reverse();
    let mut chain_config = Mapping::new();
    chain_config.insert(yaml_key("proxies"), Value::Sequence(proxies_chain));
    serde_yaml_ng::to_string(&Value::Mapping(chain_config)).context("YAML generation failed")
}

fn yaml_key(key: &str) -> Value {
    Value::String(key.to_owned())
}

fn profile_file_path(profiles_dir: &Path, file: &str) -> PathBuf {
    let path = PathBuf::from(file);
    if path.is_absolute() {
        path
    } else {
        profiles_dir.join(path)
    }
}

fn merge_default_config(config: &mut Mapping, clash: &Mapping) {
    for (key, value) in clash {
        if key.as_str() == Some("tun") {
            let mut tun = config
                .get("tun")
                .and_then(Value::as_mapping)
                .cloned()
                .unwrap_or_default();
            if let Some(patch_tun) = value.as_mapping() {
                for (key, value) in patch_tun {
                    tun.insert(key.clone(), value.clone());
                }
            }
            config.insert(key.clone(), Value::Mapping(tun));
        } else {
            config.insert(key.clone(), value.clone());
        }
    }
}

fn ensure_selectable_proxy_group(config: &mut Mapping) {
    if has_usable_proxy_group(config) {
        return;
    }

    let proxy_names = collect_proxy_names(config);
    let provider_names = collect_provider_names(config);
    if proxy_names.is_empty() && provider_names.is_empty() {
        return;
    }

    let group_name = unique_group_name(config, "Proxy");
    let mut group = Mapping::new();
    group.insert("name".into(), group_name.clone().into());
    group.insert("type".into(), "select".into());

    if !provider_names.is_empty() {
        group.insert(
            "use".into(),
            Value::Sequence(provider_names.into_iter().map(Value::String).collect()),
        );
    }

    let mut group_proxies = proxy_names.into_iter().collect::<Vec<_>>();
    if !group_proxies.iter().any(|name| name == "DIRECT") {
        group_proxies.push("DIRECT".into());
    }
    group.insert(
        "proxies".into(),
        Value::Sequence(group_proxies.into_iter().map(Value::String).collect()),
    );

    match config.get_mut("proxy-groups") {
        Some(Value::Sequence(groups)) => groups.push(Value::Mapping(group)),
        _ => {
            config.insert("proxy-groups".into(), Value::Sequence(vec![Value::Mapping(group)]));
        }
    }

    ensure_match_rule(config, &group_name);
}

fn has_usable_proxy_group(config: &Mapping) -> bool {
    config
        .get("proxy-groups")
        .and_then(Value::as_sequence)
        .is_some_and(|groups| {
            groups.iter().any(|group| {
                let Some(group) = group.as_mapping() else {
                    return false;
                };
                sequence_has_items(group, "proxies")
                    || sequence_has_items(group, "use")
                    || bool_enabled(group, "include-all")
                    || bool_enabled(group, "include-all-proxies")
                    || bool_enabled(group, "include-all-providers")
            })
        })
}

fn collect_proxy_names(config: &Mapping) -> BTreeSet<String> {
    config
        .get("proxies")
        .and_then(Value::as_sequence)
        .into_iter()
        .flatten()
        .filter_map(|proxy| match proxy {
            Value::Mapping(proxy) => proxy.get("name").and_then(Value::as_str),
            Value::String(name) => Some(name.as_str()),
            _ => None,
        })
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn collect_provider_names(config: &Mapping) -> BTreeSet<String> {
    config
        .get("proxy-providers")
        .and_then(Value::as_mapping)
        .into_iter()
        .flat_map(Mapping::keys)
        .filter_map(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn unique_group_name(config: &Mapping, base: &str) -> String {
    let existing = config
        .get("proxy-groups")
        .and_then(Value::as_sequence)
        .into_iter()
        .flatten()
        .filter_map(|group| group.as_mapping())
        .filter_map(|group| group.get("name").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();

    if !existing.contains(base) {
        return base.to_owned();
    }

    (2..)
        .map(|index| format!("{base} {index}"))
        .find(|candidate| !existing.contains(candidate.as_str()))
        .unwrap_or_else(|| base.to_owned())
}

fn ensure_match_rule(config: &mut Mapping, group_name: &str) {
    let rule = Value::String(format!("MATCH,{group_name}"));
    match config.get_mut("rules") {
        Some(Value::Sequence(rules)) => {
            if !rules.iter().any(|existing| existing == &rule) {
                rules.push(rule);
            }
        }
        _ => {
            config.insert("rules".into(), Value::Sequence(vec![rule]));
        }
    }
}

fn sequence_has_items(group: &Mapping, key: &str) -> bool {
    group
        .get(key)
        .and_then(Value::as_sequence)
        .is_some_and(|items| !items.is_empty())
}

fn bool_enabled(group: &Mapping, key: &str) -> bool {
    group.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn apply_app_settings_overrides(config: &mut Mapping, app_settings: &IAppSettings) {
    if let Some(port) = app_settings.mixed_port {
        config.insert("mixed-port".into(), port.into());
    }

    if app_settings.socks_enabled.unwrap_or(false) {
        if let Some(port) = app_settings.socks_port {
            config.insert("socks-port".into(), port.into());
        }
    } else {
        config.remove("socks-port");
    }

    if app_settings.http_enabled.unwrap_or(false) {
        if let Some(port) = app_settings.http_port {
            config.insert("port".into(), port.into());
        }
    } else {
        config.remove("port");
    }

    if app_settings.redir_enabled.unwrap_or(false) {
        if let Some(port) = app_settings.redir_port {
            config.insert("redir-port".into(), port.into());
        }
    } else {
        config.remove("redir-port");
    }

    if app_settings.tproxy_enabled.unwrap_or(false) {
        if let Some(port) = app_settings.tproxy_port {
            config.insert("tproxy-port".into(), port.into());
        }
    } else {
        config.remove("tproxy-port");
    }

    if let Some(enable_tun) = app_settings.enable_tun_mode {
        let mut tun = config
            .get("tun")
            .and_then(Value::as_mapping)
            .cloned()
            .unwrap_or_default();
        tun.insert("enable".into(), enable_tun.into());
        config.insert("tun".into(), Value::Mapping(tun));
    }
}

fn apply_controller_boundary(config: &mut Mapping, paths: &AppPaths, app_settings: &IAppSettings) {
    #[cfg(unix)]
    {
        config.remove("external-controller-cors");
        config.insert(
            "external-controller-unix".into(),
            paths.ipc_path.as_os_str().to_string_lossy().into_owned().into(),
        );
        if app_settings.enable_external_controller.unwrap_or(false) {
            let port = external_controller_port(app_settings);
            config.insert(
                "external-controller".into(),
                format!("{}:{port}", network::DEFAULT_EXTERNAL_CONTROLLER_HOST).into(),
            );
        } else {
            config.remove("external-controller");
        }
    }

    #[cfg(not(unix))]
    {
        let _ = (config, paths, app_settings);
    }
}

fn external_controller_port(app_settings: &IAppSettings) -> u16 {
    app_settings
        .external_controller_port
        .filter(|port| *port > 0)
        .unwrap_or(network::DEFAULT_EXTERNAL_CONTROLLER_PORT)
}

fn display_path(path: &Path) -> String {
    path.as_os_str().to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use serde_yaml_ng::{Mapping, Value};

    use super::{IRuntime, RuntimeConfigGenerator};
    use crate::{
        AppPaths,
        config::{ConfigStore, IAppSettings},
    };

    #[test]
    fn patch_config_updates_allowed_fields_and_tun() {
        let mut runtime = IRuntime {
            config: Some(Mapping::new()),
            ..IRuntime::default()
        };
        let mut patch = Mapping::new();
        patch.insert("allow-lan".into(), true.into());

        let mut tun = Mapping::new();
        tun.insert("Stack".into(), Value::from("system"));
        patch.insert("tun".into(), Value::from(tun));

        runtime.patch_config(&patch);
        let config = runtime.config.expect("runtime config");

        assert_eq!(config.get("allow-lan"), Some(&Value::from(true)));
        assert_eq!(
            config
                .get("tun")
                .and_then(Value::as_mapping)
                .and_then(|tun| tun.get("stack")),
            Some(&Value::from("system"))
        );
    }

    #[tokio::test]
    async fn runtime_generator_writes_fallback_config() {
        let root = temp_root("fallback");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        let loaded = store.initialize().await.expect("initialize configs");
        let generator = RuntimeConfigGenerator::from_loaded(&loaded);
        let result = generator.generate().await.expect("generate runtime");

        assert!(result.fallback);
        assert!(root.join("mihomo-runtime.yaml").is_file());
        let runtime = crate::yaml::read_mapping(root.join("mihomo-runtime.yaml"))
            .await
            .expect("read runtime");
        assert_eq!(runtime.get("mixed-port"), Some(&Value::from(7897)));
        assert!(runtime.get("socks-port").is_none());
        assert!(runtime.get("port").is_none());
        assert!(runtime.get("redir-port").is_none());
        assert!(runtime.get("tproxy-port").is_none());
        #[cfg(unix)]
        {
            assert!(runtime.get("external-controller").is_none());
            assert!(runtime.get("external-controller-unix").is_some());
        }
        #[cfg(not(unix))]
        assert_eq!(runtime.get("external-controller"), Some(&Value::from("127.0.0.1:9097")));
        assert_eq!(
            runtime
                .get("tun")
                .and_then(Value::as_mapping)
                .and_then(|tun| tun.get("enable")),
            Some(&Value::from(false))
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn runtime_generator_writes_opt_in_external_controller() {
        let root = temp_root("external-controller");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        let mut loaded = store.initialize().await.expect("initialize configs");
        loaded.app_settings = IAppSettings {
            enable_external_controller: Some(true),
            external_controller_port: Some(19097),
            ..IAppSettings::default()
        };
        loaded
            .app_settings
            .save_file(root.join("settings.yaml"))
            .await
            .expect("save app_settings");

        let generator = RuntimeConfigGenerator::from_loaded(&loaded);
        generator.generate().await.expect("generate runtime");
        let runtime = crate::yaml::read_mapping(root.join("mihomo-runtime.yaml"))
            .await
            .expect("read runtime");

        #[cfg(unix)]
        {
            assert_eq!(
                runtime.get("external-controller"),
                Some(&Value::from("127.0.0.1:19097"))
            );
            assert!(runtime.get("external-controller-cors").is_none());
            assert!(runtime.get("external-controller-unix").is_some());
        }

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn runtime_generator_defaults_external_controller_port_when_enabled_without_valid_port() {
        for (name, port) in [("missing", None), ("zero", Some(0))] {
            let root = temp_root(&format!("external-controller-default-port-{name}"));
            let _ = fs::remove_dir_all(&root);

            let store = ConfigStore::new(AppPaths::from_home(&root));
            let mut loaded = store.initialize().await.expect("initialize configs");
            loaded.app_settings = IAppSettings {
                enable_external_controller: Some(true),
                external_controller_port: port,
                ..IAppSettings::default()
            };
            loaded
                .app_settings
                .save_file(root.join("settings.yaml"))
                .await
                .expect("save app_settings");

            let generator = RuntimeConfigGenerator::from_loaded(&loaded);
            generator.generate().await.expect("generate runtime");
            let runtime = crate::yaml::read_mapping(root.join("mihomo-runtime.yaml"))
                .await
                .expect("read runtime");

            #[cfg(unix)]
            {
                assert_eq!(runtime.get("external-controller"), Some(&Value::from("127.0.0.1:9097")));
                assert!(runtime.get("external-controller-unix").is_some());
            }

            let _ = fs::remove_dir_all(&root);
        }
    }

    #[tokio::test]
    async fn runtime_generator_strips_profile_external_controller_when_disabled() {
        let root = temp_root("external-controller-disabled-profile");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        let mut loaded = store.initialize().await.expect("initialize configs");
        fs::write(
            root.join("profiles").join("demo.yaml"),
            "external-controller: 0.0.0.0:9090\nexternal-controller-cors:\n  allow-origins:\n    - '*'\nproxies: []\n",
        )
        .expect("write profile");
        loaded.profiles.current = Some("L001".into());
        loaded.profiles.items = Some(vec![crate::config::PrfItem {
            uid: Some("L001".into()),
            itype: Some("local".into()),
            file: Some("demo.yaml".into()),
            name: Some("Demo".into()),
            ..crate::config::PrfItem::default()
        }]);
        loaded
            .profiles
            .save_file(root.join("profiles.yaml"))
            .await
            .expect("save profiles");

        let generator = RuntimeConfigGenerator::from_loaded(&loaded);
        generator.generate().await.expect("generate runtime");
        let runtime = crate::yaml::read_mapping(root.join("mihomo-runtime.yaml"))
            .await
            .expect("read runtime");

        #[cfg(unix)]
        {
            assert!(runtime.get("external-controller").is_none());
            assert!(runtime.get("external-controller-cors").is_none());
            assert!(runtime.get("external-controller-unix").is_some());
        }

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn runtime_generator_overrides_profile_external_controller_when_enabled() {
        let root = temp_root("external-controller-enabled-profile");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        let mut loaded = store.initialize().await.expect("initialize configs");
        fs::write(
            root.join("profiles").join("demo.yaml"),
            "external-controller: 0.0.0.0:9090\nexternal-controller-cors:\n  allow-origins:\n    - '*'\nproxies: []\n",
        )
        .expect("write profile");
        loaded.profiles.current = Some("L001".into());
        loaded.profiles.items = Some(vec![crate::config::PrfItem {
            uid: Some("L001".into()),
            itype: Some("local".into()),
            file: Some("demo.yaml".into()),
            name: Some("Demo".into()),
            ..crate::config::PrfItem::default()
        }]);
        loaded.app_settings = IAppSettings {
            enable_external_controller: Some(true),
            external_controller_port: Some(19097),
            ..IAppSettings::default()
        };
        loaded
            .profiles
            .save_file(root.join("profiles.yaml"))
            .await
            .expect("save profiles");
        loaded
            .app_settings
            .save_file(root.join("settings.yaml"))
            .await
            .expect("save app_settings");

        let generator = RuntimeConfigGenerator::from_loaded(&loaded);
        generator.generate().await.expect("generate runtime");
        let runtime = crate::yaml::read_mapping(root.join("mihomo-runtime.yaml"))
            .await
            .expect("read runtime");

        #[cfg(unix)]
        {
            assert_eq!(
                runtime.get("external-controller"),
                Some(&Value::from("127.0.0.1:19097"))
            );
            assert!(runtime.get("external-controller-cors").is_none());
            assert!(runtime.get("external-controller-unix").is_some());
        }

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn runtime_generator_merges_dns_config_when_enabled() {
        let root = temp_root("dns-enabled");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        let mut loaded = store.initialize().await.expect("initialize configs");
        fs::write(
            root.join("dns_config.yaml"),
            "dns:\n  enable: true\n  nameserver:\n    - 1.1.1.1\nhosts:\n  example.test: 127.0.0.1\n",
        )
        .expect("write dns");
        loaded.app_settings = IAppSettings {
            enable_dns_settings: Some(true),
            ..IAppSettings::default()
        };
        loaded
            .app_settings
            .save_file(root.join("settings.yaml"))
            .await
            .expect("save app_settings");

        let generator = RuntimeConfigGenerator::from_loaded(&loaded);
        generator.generate().await.expect("generate runtime");
        let runtime = crate::yaml::read_mapping(root.join("mihomo-runtime.yaml"))
            .await
            .expect("read runtime");

        assert_eq!(
            runtime
                .get("dns")
                .and_then(Value::as_mapping)
                .and_then(|dns| dns.get("enable")),
            Some(&Value::from(true))
        );
        assert_eq!(
            runtime
                .get("hosts")
                .and_then(Value::as_mapping)
                .and_then(|hosts| hosts.get("example.test")),
            Some(&Value::from("127.0.0.1"))
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn runtime_generator_merges_unwrapped_dns_config_when_enabled() {
        let root = temp_root("dns-enabled-unwrapped");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        let mut loaded = store.initialize().await.expect("initialize configs");
        fs::write(root.join("dns_config.yaml"), "enable: true\nnameserver:\n  - 1.0.0.1\n").expect("write dns");
        loaded.app_settings = IAppSettings {
            enable_dns_settings: Some(true),
            ..IAppSettings::default()
        };
        loaded
            .app_settings
            .save_file(root.join("settings.yaml"))
            .await
            .expect("save app_settings");

        let generator = RuntimeConfigGenerator::from_loaded(&loaded);
        generator.generate().await.expect("generate runtime");
        let runtime = crate::yaml::read_mapping(root.join("mihomo-runtime.yaml"))
            .await
            .expect("read runtime");

        assert_eq!(
            runtime
                .get("dns")
                .and_then(Value::as_mapping)
                .and_then(|dns| dns.get("nameserver"))
                .and_then(Value::as_sequence)
                .and_then(|nameservers| nameservers.first()),
            Some(&Value::from("1.0.0.1"))
        );
        assert!(runtime.get("enable").is_none());

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn runtime_generator_skips_dns_config_when_disabled() {
        let root = temp_root("dns-disabled");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        let loaded = store.initialize().await.expect("initialize configs");
        fs::write(
            root.join("dns_config.yaml"),
            "dns:\n  enable: true\nhosts:\n  example.test: 127.0.0.1\n",
        )
        .expect("write dns");

        let generator = RuntimeConfigGenerator::from_loaded(&loaded);
        generator.generate().await.expect("generate runtime");
        let runtime = crate::yaml::read_mapping(root.join("mihomo-runtime.yaml"))
            .await
            .expect("read runtime");

        assert!(runtime.get("hosts").is_none());
        assert!(runtime.get("dns").is_none());

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn runtime_generator_merges_current_profile_and_http_ports() {
        let root = temp_root("profile");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        let mut loaded = store.initialize().await.expect("initialize configs");
        fs::write(
            root.join("profiles").join("demo.yaml"),
            "mode: global\nexternal-controller: 0.0.0.0:9090\nexternal-controller-cors:\n  allow-origins:\n    - '*'\nproxies:\n  - name: demo\n    type: direct\n",
        )
        .expect("write profile");
        loaded.profiles.current = Some("L001".into());
        loaded.profiles.items = Some(vec![crate::config::PrfItem {
            uid: Some("L001".into()),
            itype: Some("local".into()),
            file: Some("demo.yaml".into()),
            name: Some("Demo".into()),
            ..crate::config::PrfItem::default()
        }]);
        loaded.app_settings = IAppSettings {
            mixed_port: Some(19090),
            socks_enabled: Some(false),
            http_enabled: Some(false),
            redir_port: Some(19095),
            redir_enabled: Some(true),
            tproxy_port: Some(19096),
            tproxy_enabled: Some(true),
            enable_tun_mode: Some(true),
            enable_external_controller: Some(true),
            external_controller_port: Some(19097),
            ..IAppSettings::default()
        };
        loaded
            .profiles
            .save_file(root.join("profiles.yaml"))
            .await
            .expect("save profiles");
        loaded
            .app_settings
            .save_file(root.join("settings.yaml"))
            .await
            .expect("save app_settings");

        let generator = RuntimeConfigGenerator::from_loaded(&loaded);
        let result = generator.generate().await.expect("generate runtime");

        assert_eq!(result.source_profile.as_deref(), Some("L001"));
        assert!(!result.fallback);
        let runtime = crate::yaml::read_mapping(root.join("mihomo-runtime.yaml"))
            .await
            .expect("read runtime");
        assert_eq!(runtime.get("mode"), Some(&Value::from("rule")));
        assert_eq!(runtime.get("mixed-port"), Some(&Value::from(19090)));
        assert_eq!(runtime.get("redir-port"), Some(&Value::from(19095)));
        assert_eq!(runtime.get("tproxy-port"), Some(&Value::from(19096)));
        assert!(runtime.get("socks-port").is_none());
        assert!(runtime.get("port").is_none());
        assert_eq!(
            runtime
                .get("tun")
                .and_then(Value::as_mapping)
                .and_then(|tun| tun.get("enable")),
            Some(&Value::from(true))
        );
        assert_eq!(
            runtime
                .get("tun")
                .and_then(Value::as_mapping)
                .and_then(|tun| tun.get("auto-route")),
            Some(&Value::from(true))
        );
        assert!(runtime.get("proxies").and_then(Value::as_sequence).is_some());
        #[cfg(unix)]
        {
            assert_eq!(
                runtime.get("external-controller"),
                Some(&Value::from("127.0.0.1:19097"))
            );
            assert!(runtime.get("external-controller-cors").is_none());
            assert!(runtime.get("external-controller-unix").is_some());
        }

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn runtime_generator_creates_group_for_provider_only_profile() {
        let root = temp_root("provider-only");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        let mut loaded = store.initialize().await.expect("initialize configs");
        fs::write(
            root.join("profiles").join("provider.yaml"),
            r"proxy-providers:
  remote:
    type: http
    url: https://example.invalid/provider.yaml
    path: ./provider.yaml
rules: []
",
        )
        .expect("write profile");
        loaded.profiles.current = Some("Rprovider".into());
        loaded.profiles.items = Some(vec![crate::config::PrfItem {
            uid: Some("Rprovider".into()),
            itype: Some("remote".into()),
            file: Some("provider.yaml".into()),
            name: Some("Provider Only".into()),
            ..crate::config::PrfItem::default()
        }]);
        loaded
            .profiles
            .save_file(root.join("profiles.yaml"))
            .await
            .expect("save profiles");

        let generator = RuntimeConfigGenerator::from_loaded(&loaded);
        generator.generate().await.expect("generate runtime");
        let runtime = crate::yaml::read_mapping(root.join("mihomo-runtime.yaml"))
            .await
            .expect("read runtime");

        let group = runtime
            .get("proxy-groups")
            .and_then(Value::as_sequence)
            .and_then(|groups| groups.first())
            .and_then(Value::as_mapping)
            .expect("generated proxy group");
        assert_eq!(group.get("name").and_then(Value::as_str), Some("Proxy"));
        assert_eq!(group.get("type").and_then(Value::as_str), Some("select"));
        assert_eq!(
            group
                .get("use")
                .and_then(Value::as_sequence)
                .and_then(|providers| providers.first())
                .and_then(Value::as_str),
            Some("remote")
        );
        assert_eq!(
            group
                .get("proxies")
                .and_then(Value::as_sequence)
                .and_then(|proxies| proxies.first())
                .and_then(Value::as_str),
            Some("DIRECT")
        );
        assert!(
            runtime
                .get("rules")
                .and_then(Value::as_sequence)
                .is_some_and(|rules| rules.iter().any(|rule| rule.as_str() == Some("MATCH,Proxy")))
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn runtime_generator_creates_group_for_proxies_without_groups() {
        let root = temp_root("proxies-without-groups");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        let mut loaded = store.initialize().await.expect("initialize configs");
        fs::write(
            root.join("profiles").join("proxies.yaml"),
            r"proxies:
  - name: HK-1
    type: direct
  - name: SG-1
    type: direct
",
        )
        .expect("write profile");
        loaded.profiles.current = Some("Rproxies".into());
        loaded.profiles.items = Some(vec![crate::config::PrfItem {
            uid: Some("Rproxies".into()),
            itype: Some("remote".into()),
            file: Some("proxies.yaml".into()),
            name: Some("Proxies".into()),
            ..crate::config::PrfItem::default()
        }]);
        loaded
            .profiles
            .save_file(root.join("profiles.yaml"))
            .await
            .expect("save profiles");

        let generator = RuntimeConfigGenerator::from_loaded(&loaded);
        generator.generate().await.expect("generate runtime");
        let runtime = crate::yaml::read_mapping(root.join("mihomo-runtime.yaml"))
            .await
            .expect("read runtime");

        let group_proxies = runtime
            .get("proxy-groups")
            .and_then(Value::as_sequence)
            .and_then(|groups| groups.first())
            .and_then(Value::as_mapping)
            .and_then(|group| group.get("proxies"))
            .and_then(Value::as_sequence)
            .expect("generated group proxies")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        assert_eq!(group_proxies, vec!["HK-1", "SG-1", "DIRECT"]);

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn runtime_generator_keeps_existing_usable_group() {
        let root = temp_root("existing-group");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        let mut loaded = store.initialize().await.expect("initialize configs");
        fs::write(
            root.join("profiles").join("group.yaml"),
            r"proxies:
  - name: HK-1
    type: direct
proxy-groups:
  - name: Manual
    type: select
    proxies:
      - HK-1
rules:
  - MATCH,Manual
",
        )
        .expect("write profile");
        loaded.profiles.current = Some("Rgroup".into());
        loaded.profiles.items = Some(vec![crate::config::PrfItem {
            uid: Some("Rgroup".into()),
            itype: Some("remote".into()),
            file: Some("group.yaml".into()),
            name: Some("Group".into()),
            ..crate::config::PrfItem::default()
        }]);
        loaded
            .profiles
            .save_file(root.join("profiles.yaml"))
            .await
            .expect("save profiles");

        let generator = RuntimeConfigGenerator::from_loaded(&loaded);
        generator.generate().await.expect("generate runtime");
        let runtime = crate::yaml::read_mapping(root.join("mihomo-runtime.yaml"))
            .await
            .expect("read runtime");

        let groups = runtime
            .get("proxy-groups")
            .and_then(Value::as_sequence)
            .expect("proxy groups");
        assert_eq!(groups.len(), 1);
        assert_eq!(
            groups
                .first()
                .and_then(Value::as_mapping)
                .and_then(|group| group.get("name"))
                .and_then(Value::as_str),
            Some("Manual")
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn runtime_generator_applies_proxy_chain_override() {
        let root = temp_root("proxy-chain");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        let mut loaded = store.initialize().await.expect("initialize configs");
        fs::write(
            root.join("profiles").join("chain.yaml"),
            "proxies:\n  - name: Proxy A\n    type: direct\n  - name: Proxy B\n    type: direct\nrules: []\n",
        )
        .expect("write profile");
        loaded.profiles.current = Some("Lchain".into());
        loaded.profiles.items = Some(vec![crate::config::PrfItem {
            uid: Some("Lchain".into()),
            itype: Some("local".into()),
            file: Some("chain.yaml".into()),
            name: Some("Chain".into()),
            ..crate::config::PrfItem::default()
        }]);
        loaded
            .profiles
            .save_file(root.join("profiles.yaml"))
            .await
            .expect("save profiles");

        let generator = RuntimeConfigGenerator::from_loaded(&loaded);
        generator
            .set_proxy_chain_config(Some(vec!["Proxy A".into(), "Proxy B".into()]))
            .expect("set proxy chain");
        generator.generate().await.expect("generate runtime");

        let runtime = crate::yaml::read_mapping(root.join("mihomo-runtime.yaml"))
            .await
            .expect("read runtime");
        let proxy_b = runtime
            .get("proxies")
            .and_then(Value::as_sequence)
            .and_then(|proxies| {
                proxies.iter().find(|proxy| {
                    proxy
                        .as_mapping()
                        .and_then(|proxy| proxy.get("name"))
                        .and_then(Value::as_str)
                        == Some("Proxy B")
                })
            })
            .and_then(Value::as_mapping)
            .expect("Proxy B mapping");
        assert_eq!(proxy_b.get("dialer-proxy").and_then(Value::as_str), Some("Proxy A"));

        let chain_yaml = generator
            .read_proxy_chain_yaml("Proxy B")
            .await
            .expect("read proxy chain yaml");
        assert!(chain_yaml.contains("Proxy A"));
        assert!(chain_yaml.contains("Proxy B"));

        generator.set_proxy_chain_config(None).expect("clear proxy chain");
        generator.generate().await.expect("regenerate runtime");
        let runtime = crate::yaml::read_mapping(root.join("mihomo-runtime.yaml"))
            .await
            .expect("read runtime");
        assert!(
            runtime
                .get("proxies")
                .and_then(Value::as_sequence)
                .into_iter()
                .flatten()
                .filter_map(Value::as_mapping)
                .all(|proxy| proxy.get("dialer-proxy").is_none())
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn runtime_generator_applies_global_script_to_current_profile() {
        let root = temp_root("global-script");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        let mut loaded = store.initialize().await.expect("initialize configs");
        fs::write(
            root.join("profiles").join("demo.yaml"),
            r"proxies:
  - name: Existing
    type: direct
proxy-groups:
  - name: Existing Group
    type: select
    proxies:
      - Existing
rules:
  - MATCH,Existing Group
",
        )
        .expect("write profile");
        fs::write(root.join("profiles").join("Script.js"), LAN_ACCESS_SCRIPT).expect("write global script");
        loaded.profiles.current = Some("L001".into());
        loaded.profiles.items = Some(vec![
            crate::config::PrfItem {
                uid: Some("Merge".into()),
                itype: Some("merge".into()),
                name: Some("Merge".into()),
                file: Some("Merge.yaml".into()),
                ..crate::config::PrfItem::default()
            },
            crate::config::PrfItem {
                uid: Some("Script".into()),
                itype: Some("script".into()),
                name: Some("Script".into()),
                file: Some("Script.js".into()),
                ..crate::config::PrfItem::default()
            },
            crate::config::PrfItem {
                uid: Some("L001".into()),
                itype: Some("local".into()),
                file: Some("demo.yaml".into()),
                name: Some("Demo".into()),
                ..crate::config::PrfItem::default()
            },
        ]);
        loaded
            .profiles
            .save_file(root.join("profiles.yaml"))
            .await
            .expect("save profiles");

        let generator = RuntimeConfigGenerator::from_loaded(&loaded);
        generator.generate().await.expect("generate runtime");

        let runtime = crate::yaml::read_mapping(root.join("mihomo-runtime.yaml"))
            .await
            .expect("read runtime");
        let proxies = runtime
            .get("proxies")
            .and_then(Value::as_sequence)
            .expect("proxies should be present");
        assert!(proxies.iter().any(|proxy| {
            proxy
                .as_mapping()
                .and_then(|proxy| proxy.get("name"))
                .and_then(Value::as_str)
                == Some("EasyTier-LAN")
        }));

        let groups = runtime
            .get("proxy-groups")
            .and_then(Value::as_sequence)
            .expect("proxy-groups should be present");
        assert!(groups.iter().any(|group| {
            group
                .as_mapping()
                .and_then(|group| group.get("name"))
                .and_then(Value::as_str)
                == Some("LAN-Access")
        }));

        let first_rule = runtime
            .get("rules")
            .and_then(Value::as_sequence)
            .and_then(|rules| rules.first())
            .and_then(Value::as_str);
        assert_eq!(first_rule, Some("IP-CIDR,192.168.4.0/24,LAN-Access,no-resolve"));

        let _ = fs::remove_dir_all(&root);
    }

    const LAN_ACCESS_SCRIPT: &str = r#"
function main(config) {
  const proxyName = "EasyTier-LAN";
  const groupName = "LAN-Access";
  const lanCidr = "192.168.4.0/24";

  config.proxies = Array.isArray(config.proxies) ? config.proxies : [];
  config["proxy-groups"] = Array.isArray(config["proxy-groups"]) ? config["proxy-groups"] : [];
  config.rules = Array.isArray(config.rules) ? config.rules : [];

  config.proxies = config.proxies.filter((p) => p?.name !== proxyName);
  config.proxies.push({
    name: proxyName,
    type: "direct"
  });

  config["proxy-groups"] = config["proxy-groups"].filter((g) => g?.name !== groupName);
  config["proxy-groups"].push({
    name: groupName,
    type: "select",
    proxies: [proxyName, "DIRECT"]
  });

  for (const group of config["proxy-groups"]) {
    if (!group || group.name === groupName) continue;

    if (Array.isArray(group.proxies)) {
      group.proxies = group.proxies.filter((name) => name !== proxyName);
    }

    if (group["include-all"]) {
      const exclude = `^${proxyName}$`;
      const old = String(group["exclude-filter"] || "");
      group["exclude-filter"] = old && !old.includes(proxyName)
        ? `${old}|${exclude}`
        : (old || exclude);
    }
  }

  config.rules = config.rules.filter((rule) => {
    const text = String(rule);
    return !text.includes(lanCidr) &&
      !text.includes(proxyName) &&
      !text.includes(groupName);
  });

  config.rules = [
    `IP-CIDR,${lanCidr},${groupName},no-resolve`,
    ...config.rules
  ];

  return config;
}
"#;

    fn temp_root(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        std::env::temp_dir().join(format!("clash-core-runtime-{name}-{}-{nanos}", std::process::id()))
    }
}
