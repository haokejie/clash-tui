use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{
    AppPathSummary, AppPaths,
    config::{
        ClashInfo, IAppSettings, IClashTemp, IProfiles, LocalProfileImport, PrfItem, PrfOption, RemoteProfileImport,
        dns,
        profiles::{
            GLOBAL_PROFILE_DEFAULTS, RemoteProfileDownload, download_remote_profile, generate_local_uid,
            generate_remote_uid, remote_profile_name_override_for_update, validate_profile_uid, validate_profile_yaml,
        },
    },
    validation::{ValidationOutcome, ValidationSkipReason},
    yaml,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigFile {
    Clash,
    Settings,
    Profiles,
    Dns,
}

#[derive(Debug, Clone)]
pub struct ConfigLoadResult {
    pub paths: AppPaths,
    pub created_files: Vec<ConfigFile>,
    pub clash: IClashTemp,
    pub app_settings: IAppSettings,
    pub profiles: IProfiles,
}

impl ConfigLoadResult {
    #[must_use]
    pub fn path_summary(&self) -> AppPathSummary {
        AppPathSummary::from(&self.paths)
    }

    #[must_use]
    pub fn clash_info(&self) -> ClashInfo {
        self.clash.get_client_info()
    }
}

#[derive(Debug, Clone)]
pub struct ConfigStore {
    paths: AppPaths,
}

#[derive(Debug, Clone)]
pub struct RemoteProfileUpdatePlan {
    pub uid: String,
    pub request: RemoteProfileImport,
    pub option: Option<PrfOption>,
}

impl ConfigStore {
    #[must_use]
    pub const fn new(paths: AppPaths) -> Self {
        Self { paths }
    }

    #[must_use]
    pub const fn paths(&self) -> &AppPaths {
        &self.paths
    }

    pub async fn initialize(&self) -> Result<ConfigLoadResult> {
        self.paths.ensure_dirs()?;

        let mut created_files = Vec::new();
        let clash = self.ensure_clash(&mut created_files).await?;
        let app_settings = self.ensure_app_settings(&mut created_files).await?;
        let profiles = self.ensure_profiles(&mut created_files).await?;
        self.ensure_dns(&mut created_files).await?;

        Ok(ConfigLoadResult {
            paths: self.paths.clone(),
            created_files,
            clash,
            app_settings,
            profiles,
        })
    }

    pub async fn load_app_settings(&self) -> Result<IAppSettings> {
        yaml::read_yaml(&self.paths.settings_config)
            .await
            .with_context(|| format!("failed to load {}", self.paths.settings_config.display()))
    }

    pub async fn load_clash(&self) -> Result<IClashTemp> {
        let mapping = yaml::read_mapping(&self.paths.clash_config)
            .await
            .with_context(|| format!("failed to load {}", self.paths.clash_config.display()))?;
        Ok(IClashTemp::from_mapping(mapping, Some(&self.paths.ipc_path)))
    }

    pub async fn patch_clash(&self, patch: &serde_yaml_ng::Mapping) -> Result<IClashTemp> {
        self.paths.ensure_dirs()?;
        let mut clash = IClashTemp::load_or_template(&self.paths.clash_config, Some(&self.paths.ipc_path)).await;
        clash.patch_config(patch);
        clash.save_config(&self.paths.clash_config).await?;
        Ok(clash)
    }

    pub async fn patch_app_settings(&self, patch: &IAppSettings) -> Result<IAppSettings> {
        self.paths.ensure_dirs()?;
        let mut app_settings = self.load_app_settings().await.unwrap_or_default();
        app_settings.patch_config(patch);
        app_settings.save_file(&self.paths.settings_config).await?;
        Ok(app_settings)
    }

    pub async fn load_profiles(&self) -> Result<IProfiles> {
        yaml::read_yaml(&self.paths.profiles_config)
            .await
            .with_context(|| format!("failed to load {}", self.paths.profiles_config.display()))
    }

    pub async fn switch_profile(&self, uid: &str) -> Result<IProfiles> {
        let mut profiles = self.load_profiles().await?;
        profiles.switch_current(uid)?;
        profiles.save_file(&self.paths.profiles_config).await?;
        Ok(profiles)
    }

    pub async fn patch_profile(&self, uid: &str, patch: &PrfItem) -> Result<IProfiles> {
        self.paths.ensure_dirs()?;
        validate_profile_uid(uid)?;
        let mut profiles = self.load_profiles().await?;
        profiles.patch_item(uid, patch)?;
        profiles.save_file(&self.paths.profiles_config).await?;
        Ok(profiles)
    }

    pub async fn reorder_profiles(&self, active_id: &str, over_id: &str) -> Result<IProfiles> {
        self.paths.ensure_dirs()?;
        validate_profile_uid(active_id)?;
        validate_profile_uid(over_id)?;

        let mut profiles = self.load_profiles().await?;
        let items = profiles.items.get_or_insert_with(Vec::new);
        let old_index = items
            .iter()
            .position(|item| item.uid.as_deref() == Some(active_id))
            .with_context(|| format!("failed to find active profile \"uid:{active_id}\""))?;
        let new_index = items
            .iter()
            .position(|item| item.uid.as_deref() == Some(over_id))
            .with_context(|| format!("failed to find target profile \"uid:{over_id}\""))?;

        if old_index != new_index {
            let item = items.remove(old_index);
            items.insert(new_index, item);
            profiles.save_file(&self.paths.profiles_config).await?;
        }

        Ok(profiles)
    }

    pub async fn delete_profile(&self, uid: &str) -> Result<IProfiles> {
        self.paths.ensure_dirs()?;
        validate_profile_uid(uid)?;

        let mut profiles = self.load_profiles().await?;
        let current = profiles.current.clone();
        let delete_uids = {
            let item = profiles.get_item(uid)?;
            item.option.as_ref().map_or_else(Vec::new, |option| {
                [
                    option.merge.clone(),
                    option.script.clone(),
                    option.rules.clone(),
                    option.proxies.clone(),
                    option.groups.clone(),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
            })
        };

        let mut files = Vec::new();
        let removed_main = remove_profile_item(&mut profiles, uid, &mut files);
        for delete_uid in delete_uids {
            let _ = remove_profile_item(&mut profiles, &delete_uid, &mut files);
        }
        if !removed_main {
            bail!("failed to find the profile item \"uid:{uid}\"");
        }

        if current.as_deref() == Some(uid) {
            profiles.current = profiles.items.as_deref().and_then(|items| {
                items
                    .iter()
                    .find(|item| matches!(item.itype.as_deref(), Some("remote" | "local")))
                    .and_then(|item| item.uid.clone())
            });
        }

        let paths = files
            .iter()
            .map(|file| self.profile_file_path(file))
            .collect::<Result<Vec<_>>>()?;

        profiles.save_file(&self.paths.profiles_config).await?;
        for path in paths {
            match tokio::fs::remove_file(&path).await {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err).with_context(|| format!("failed to remove profile {}", path.display())),
            }
        }

        Ok(profiles)
    }

    pub async fn read_profile_file(&self, uid: &str) -> Result<String> {
        self.paths.ensure_dirs()?;
        validate_profile_uid(uid)?;

        let profiles = self.load_profiles().await?;
        let item = profiles.get_item(uid)?;
        let file = item.file.as_deref().context("profile file field is missing")?;
        let path = self.profile_file_path(file)?;
        tokio::fs::read_to_string(&path)
            .await
            .with_context(|| format!("failed to read profile {}", path.display()))
    }

    pub async fn save_profile_file(&self, uid: &str, file_data: &str) -> Result<ValidationOutcome> {
        self.paths.ensure_dirs()?;
        validate_profile_uid(uid)?;

        let profiles = self.load_profiles().await?;
        let item = profiles.get_item(uid)?;
        let file = item.file.as_deref().context("profile file field is missing")?;
        let path = self.profile_file_path(file)?;
        let is_script = item.itype.as_deref() == Some("script") || file.ends_with(".js");

        if file_data.is_empty() {
            return Ok(ValidationOutcome::Skipped {
                reason: ValidationSkipReason::Debounced,
            });
        }

        if !is_script && let Err(err) = validate_profile_yaml(file_data) {
            return Ok(ValidationOutcome::invalid_from_message(err.to_string()));
        }

        tokio::fs::write(&path, file_data)
            .await
            .with_context(|| format!("failed to write profile {}", path.display()))?;
        Ok(ValidationOutcome::Valid)
    }

    pub async fn import_local_profile(&self, input: &LocalProfileImport) -> Result<IProfiles> {
        self.paths.ensure_dirs()?;
        validate_profile_yaml(&input.file_data)?;

        let mut profiles = self.load_profiles().await.unwrap_or_default();
        let uid = match input.uid.as_deref().filter(|uid| !uid.trim().is_empty()) {
            Some(uid) => uid.to_owned(),
            None => generate_local_uid(),
        };
        validate_profile_uid(&uid)?;
        if profiles.get_item(&uid).is_ok() {
            bail!("the profile item \"uid:{uid}\" already exists");
        }

        let file = format!("{uid}.yaml");
        let path = self.paths.profiles_dir.join(&file);
        if path_exists(&path).await {
            bail!("the profile file already exists: {}", path.display());
        }

        tokio::fs::write(&path, &input.file_data)
            .await
            .with_context(|| format!("failed to write profile {}", path.display()))?;

        profiles.append_metadata(PrfItem {
            uid: Some(uid),
            itype: Some("local".into()),
            name: input.name.clone().or_else(|| Some("Local Profile".into())),
            file: Some(file),
            updated: Some(current_timestamp_secs()),
            ..PrfItem::default()
        })?;
        profiles.save_file(&self.paths.profiles_config).await?;
        Ok(profiles)
    }

    pub async fn import_remote_profile(&self, input: &RemoteProfileImport) -> Result<IProfiles> {
        self.paths.ensure_dirs()?;
        let remote = download_remote_profile(input).await?;
        self.commit_remote_profile_import(input, remote).await
    }

    pub async fn commit_remote_profile_import(
        &self,
        input: &RemoteProfileImport,
        remote: RemoteProfileDownload,
    ) -> Result<IProfiles> {
        self.paths.ensure_dirs()?;
        let mut profiles = self.load_profiles().await.unwrap_or_default();
        let uid = match input.uid.as_deref().filter(|uid| !uid.trim().is_empty()) {
            Some(uid) => uid.to_owned(),
            None => generate_remote_uid(),
        };
        validate_profile_uid(&uid)?;
        if profiles.get_item(&uid).is_ok() {
            bail!("the profile item \"uid:{uid}\" already exists");
        }

        let file = format!("{uid}.yaml");
        let path = self.paths.profiles_dir.join(&file);
        if path_exists(&path).await {
            bail!("the profile file already exists: {}", path.display());
        }

        tokio::fs::write(&path, &remote.file_data)
            .await
            .with_context(|| format!("failed to write profile {}", path.display()))?;

        profiles.append_metadata(PrfItem {
            uid: Some(uid),
            itype: Some("remote".into()),
            name: Some(remote.name),
            desc: input.desc.clone(),
            file: Some(file),
            url: Some(remote.url),
            extra: remote.extra,
            updated: Some(current_timestamp_secs()),
            option: Some(merge_remote_option(input.option.as_ref(), remote.update_interval)),
            home: remote.home,
            ..PrfItem::default()
        })?;
        profiles.save_file(&self.paths.profiles_config).await?;
        Ok(profiles)
    }

    pub async fn update_remote_profile(&self, uid: &str, option: Option<&PrfOption>) -> Result<IProfiles> {
        let plan = self.prepare_remote_profile_update(uid, option).await?;
        let remote = download_remote_profile(&plan.request).await?;
        self.commit_remote_profile_update(&plan, remote).await
    }

    pub async fn prepare_remote_profile_update(
        &self,
        uid: &str,
        option: Option<&PrfOption>,
    ) -> Result<RemoteProfileUpdatePlan> {
        self.paths.ensure_dirs()?;
        let profiles = self.load_profiles().await?;
        let item = profiles.get_item(uid)?.clone();
        if item.itype.as_deref() != Some("remote") {
            bail!("profile \"uid:{uid}\" is not remote");
        }
        let url = item.url.clone().context("remote profile url is missing")?;
        let option = PrfOption::merge(item.option.as_ref(), option);
        Ok(RemoteProfileUpdatePlan {
            uid: uid.to_owned(),
            request: RemoteProfileImport {
                url: url.clone(),
                uid: Some(uid.to_owned()),
                name: remote_profile_name_override_for_update(item.name.as_deref(), &url),
                desc: item.desc.clone(),
                option: option.clone(),
            },
            option,
        })
    }

    pub async fn commit_remote_profile_update(
        &self,
        plan: &RemoteProfileUpdatePlan,
        remote: RemoteProfileDownload,
    ) -> Result<IProfiles> {
        self.paths.ensure_dirs()?;
        let mut profiles = self.load_profiles().await?;
        let item = profiles.get_item(&plan.uid)?.clone();
        if item.itype.as_deref() != Some("remote") {
            bail!("profile \"uid:{}\" is not remote", plan.uid);
        }
        let file = item.file.clone().unwrap_or_else(|| format!("{}.yaml", plan.uid));
        let path = self.paths.profiles_dir.join(&file);
        tokio::fs::write(&path, &remote.file_data)
            .await
            .with_context(|| format!("failed to write profile {}", path.display()))?;

        profiles.patch_item(
            &plan.uid,
            &PrfItem {
                name: Some(remote.name),
                file: Some(file),
                url: Some(remote.url),
                extra: remote.extra,
                updated: Some(current_timestamp_secs()),
                option: Some(merge_remote_option(plan.option.as_ref(), remote.update_interval)),
                home: remote.home,
                ..PrfItem::default()
            },
        )?;
        profiles.save_file(&self.paths.profiles_config).await?;
        Ok(profiles)
    }

    fn profile_file_path(&self, file: &str) -> Result<PathBuf> {
        let relative = Path::new(file);
        if file.trim().is_empty() || relative.is_absolute() {
            bail!("invalid profile file path");
        }

        let mut components = relative.components();
        let Some(Component::Normal(_)) = components.next() else {
            bail!("invalid profile file path");
        };
        if components.next().is_some() {
            bail!("profile file path must not contain directories");
        }

        Ok(self.paths.profiles_dir.join(relative))
    }

    async fn ensure_clash(&self, created_files: &mut Vec<ConfigFile>) -> Result<IClashTemp> {
        if !path_exists(&self.paths.clash_config).await {
            let clash = IClashTemp::template_with_ipc(Some(&self.paths.ipc_path));
            clash.save_config(&self.paths.clash_config).await?;
            created_files.push(ConfigFile::Clash);
            return Ok(clash);
        }

        let mapping = yaml::read_mapping(&self.paths.clash_config)
            .await
            .with_context(|| format!("failed to load {}", self.paths.clash_config.display()))?;
        Ok(IClashTemp::from_mapping(mapping, Some(&self.paths.ipc_path)))
    }

    async fn ensure_app_settings(&self, created_files: &mut Vec<ConfigFile>) -> Result<IAppSettings> {
        if !path_exists(&self.paths.settings_config).await {
            let app_settings = IAppSettings::default();
            app_settings.save_file(&self.paths.settings_config).await?;
            created_files.push(ConfigFile::Settings);
            return Ok(app_settings);
        }

        yaml::read_yaml(&self.paths.settings_config)
            .await
            .with_context(|| format!("failed to load {}", self.paths.settings_config.display()))
    }

    async fn ensure_profiles(&self, created_files: &mut Vec<ConfigFile>) -> Result<IProfiles> {
        let mut created_profiles_config = false;
        let mut profiles = if !path_exists(&self.paths.profiles_config).await {
            created_profiles_config = true;
            IProfiles::default()
        } else {
            yaml::read_yaml(&self.paths.profiles_config)
                .await
                .with_context(|| format!("failed to load {}", self.paths.profiles_config.display()))?
        };

        let mut changed = profiles.ensure_global_profile_items();
        for default in GLOBAL_PROFILE_DEFAULTS {
            let file = {
                let items = profiles.items.get_or_insert_with(Vec::new);
                let item = items
                    .iter_mut()
                    .find(|item| item.uid.as_deref() == Some(default.uid))
                    .with_context(|| format!("failed to initialize global profile {}", default.uid))?;

                let needs_default_file = match item.file.as_deref() {
                    Some(file) => self.profile_file_path(file).is_err(),
                    None => true,
                };

                if needs_default_file {
                    item.file = Some(default.file.to_owned());
                    changed = true;
                }

                item.file.clone().unwrap_or_else(|| default.file.to_owned())
            };

            let path = self.profile_file_path(&file)?;
            if !path_exists(&path).await {
                tokio::fs::write(&path, default.file_data)
                    .await
                    .with_context(|| format!("failed to initialize profile {}", path.display()))?;
            }
        }

        if created_profiles_config || changed {
            profiles.save_file(&self.paths.profiles_config).await?;
        }

        if created_profiles_config {
            created_files.push(ConfigFile::Profiles);
        }

        Ok(profiles)
    }

    async fn ensure_dns(&self, created_files: &mut Vec<ConfigFile>) -> Result<()> {
        if dns::ensure_dns_config(&self.paths.dns_config).await? {
            created_files.push(ConfigFile::Dns);
        }
        Ok(())
    }
}

fn current_timestamp_secs() -> usize {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .try_into()
        .unwrap_or(usize::MAX)
}

fn merge_remote_option(option: Option<&PrfOption>, update_interval: Option<u64>) -> PrfOption {
    let mut option = option.cloned().unwrap_or_default();
    option.update_interval = update_interval.or(option.update_interval);
    option.allow_auto_update = option.allow_auto_update.or(Some(true));
    option
}

async fn path_exists(path: &Path) -> bool {
    tokio::fs::try_exists(path).await.unwrap_or(false)
}

fn remove_profile_item(profiles: &mut IProfiles, uid: &str, files: &mut Vec<String>) -> bool {
    let Some(items) = profiles.items.as_mut() else {
        return false;
    };
    let Some(index) = items.iter().position(|item| item.uid.as_deref() == Some(uid)) else {
        return false;
    };
    let item = items.remove(index);
    if let Some(file) = item.file {
        files.push(file);
    }
    true
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{ConfigFile, ConfigStore};
    use anyhow::Result;
    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::TcpListener,
    };

    use crate::{
        AppPaths, ValidationOutcome,
        config::{LocalProfileImport, PrfItem, PrfSelected, RemoteProfileImport},
    };

    #[tokio::test]
    async fn initialize_creates_minimum_config_files() {
        let root = temp_root("init");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        let loaded = store.initialize().await.expect("initialize configs");

        assert_eq!(
            loaded.created_files,
            vec![
                ConfigFile::Clash,
                ConfigFile::Settings,
                ConfigFile::Profiles,
                ConfigFile::Dns
            ]
        );
        assert!(root.join("config.yaml").is_file());
        assert!(root.join("settings.yaml").is_file());
        assert!(root.join("profiles.yaml").is_file());
        assert!(root.join("dns_config.yaml").is_file());
        assert!(root.join("profiles").join("Merge.yaml").is_file());
        assert!(root.join("profiles").join("Script.js").is_file());
        assert_eq!(loaded.clash_info().server, "127.0.0.1:9097");
        assert_eq!(
            loaded
                .profiles
                .get_item("Merge")
                .expect("global merge")
                .itype
                .as_deref(),
            Some("merge")
        );
        assert_eq!(
            loaded
                .profiles
                .get_item("Script")
                .expect("global script")
                .itype
                .as_deref(),
            Some("script")
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn initialize_loads_existing_config_without_recreating() {
        let root = temp_root("load");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        let first = store.initialize().await.expect("first initialize");
        let second = store.initialize().await.expect("second initialize");

        assert!(!first.created_files.is_empty());
        assert!(second.created_files.is_empty());

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn initialize_repairs_missing_global_profile_files() {
        let root = temp_root("global-profiles");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        store.initialize().await.expect("initial initialize");
        fs::write(
            root.join("profiles.yaml"),
            "current: L001\nitems:\n- uid: Merge\n  type: local\n  name: Legacy Merge\n",
        )
        .expect("write legacy profiles");
        let _ = fs::remove_file(root.join("profiles").join("Merge.yaml"));
        let _ = fs::remove_file(root.join("profiles").join("Script.js"));

        let loaded = store.initialize().await.expect("repair profiles");

        assert!(loaded.created_files.is_empty());
        assert!(root.join("profiles").join("Merge.yaml").is_file());
        assert!(root.join("profiles").join("Script.js").is_file());
        assert_eq!(
            loaded.profiles.get_item("Merge").expect("global merge").file.as_deref(),
            Some("Merge.yaml")
        );
        assert_eq!(
            loaded
                .profiles
                .get_item("Merge")
                .expect("global merge")
                .itype
                .as_deref(),
            Some("merge")
        );
        assert_eq!(
            loaded
                .profiles
                .get_item("Script")
                .expect("global script")
                .file
                .as_deref(),
            Some("Script.js")
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn import_local_profile_writes_file_and_switches_current() {
        let root = temp_root("profile");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        store.initialize().await.expect("initialize configs");
        let profiles = store
            .import_local_profile(&LocalProfileImport {
                uid: Some("L001".into()),
                name: Some("Demo".into()),
                file_data: "mode: global\nproxies: []\n".into(),
            })
            .await
            .expect("import profile");

        assert_eq!(profiles.current.as_deref(), Some("L001"));
        assert!(root.join("profiles").join("L001.yaml").is_file());

        let switched = store.switch_profile("L001").await.expect("switch profile");
        assert_eq!(switched.current.as_deref(), Some("L001"));

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn patch_profile_persists_metadata() {
        let root = temp_root("profile-patch");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        store.initialize().await.expect("initialize configs");
        store
            .import_local_profile(&LocalProfileImport {
                uid: Some("L001".into()),
                name: Some("Demo".into()),
                file_data: "mode: global\nproxies: []\n".into(),
            })
            .await
            .expect("import profile");

        let profiles = store
            .patch_profile(
                "L001",
                &PrfItem {
                    selected: Some(vec![PrfSelected {
                        name: Some("GLOBAL".into()),
                        now: Some("DIRECT".into()),
                    }]),
                    ..PrfItem::default()
                },
            )
            .await
            .expect("patch profile");

        let item = profiles.get_item("L001").expect("profile item");
        assert_eq!(
            item.selected.as_ref().expect("selected")[0].name.as_deref(),
            Some("GLOBAL")
        );

        let loaded = store.load_profiles().await.expect("load profiles");
        assert_eq!(
            loaded
                .get_item("L001")
                .expect("profile item")
                .selected
                .as_ref()
                .expect("selected")[0]
                .now
                .as_deref(),
            Some("DIRECT"),
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn read_save_and_reorder_profile_files() {
        let root = temp_root("profile-file");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        store.initialize().await.expect("initialize configs");
        store
            .import_local_profile(&LocalProfileImport {
                uid: Some("L001".into()),
                name: Some("One".into()),
                file_data: "mode: global\nproxies: []\n".into(),
            })
            .await
            .expect("import first profile");
        store
            .import_local_profile(&LocalProfileImport {
                uid: Some("L002".into()),
                name: Some("Two".into()),
                file_data: "mode: rule\nproxies: []\n".into(),
            })
            .await
            .expect("import second profile");

        let original = store.read_profile_file("L001").await.expect("read profile");
        assert!(original.contains("mode: global"));

        let outcome = store
            .save_profile_file("L001", "mode: direct\nproxies: []\n")
            .await
            .expect("save profile");
        assert_eq!(outcome, ValidationOutcome::Valid);
        assert!(
            store
                .read_profile_file("L001")
                .await
                .expect("read saved profile")
                .contains("mode: direct")
        );

        let invalid = store
            .save_profile_file("L001", "not: [valid")
            .await
            .expect("invalid profile returns outcome");
        assert!(matches!(invalid, ValidationOutcome::Invalid { .. }));
        assert!(
            store
                .read_profile_file("L001")
                .await
                .expect("read after invalid save")
                .contains("mode: direct")
        );

        let reordered = store.reorder_profiles("L002", "L001").await.expect("reorder profiles");
        let local_uids = reordered
            .items
            .as_deref()
            .expect("items")
            .iter()
            .filter(|item| matches!(item.itype.as_deref(), Some("local" | "remote")))
            .filter_map(|item| item.uid.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(local_uids, vec!["L002", "L001"]);

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn delete_profile_removes_file_and_selects_next_current() {
        let root = temp_root("profile-delete");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        store.initialize().await.expect("initialize configs");
        store
            .import_local_profile(&LocalProfileImport {
                uid: Some("L001".into()),
                name: Some("One".into()),
                file_data: "mode: global\nproxies: []\n".into(),
            })
            .await
            .expect("import first profile");
        store
            .import_local_profile(&LocalProfileImport {
                uid: Some("L002".into()),
                name: Some("Two".into()),
                file_data: "mode: rule\nproxies: []\n".into(),
            })
            .await
            .expect("import second profile");

        let profiles = store.delete_profile("L001").await.expect("delete profile");
        assert_eq!(profiles.current.as_deref(), Some("L002"));
        assert!(profiles.get_item("L001").is_err());
        assert!(!root.join("profiles").join("L001.yaml").exists());
        assert!(root.join("profiles").join("L002.yaml").is_file());

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn patch_app_settings_persists_only_present_fields() {
        let root = temp_root("app_settings");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        store.initialize().await.expect("initialize configs");
        let app_settings = store
            .patch_app_settings(&crate::config::IAppSettings {
                mixed_port: Some(19090),
                enable_auto_launch: Some(true),
                enable_system_proxy: Some(true),
                proxy_host: Some("127.0.0.1".into()),
                ..crate::config::IAppSettings::default()
            })
            .await
            .expect("patch app_settings");

        assert_eq!(app_settings.mixed_port, Some(19090));
        assert_eq!(app_settings.enable_auto_launch, Some(true));
        assert_eq!(app_settings.enable_system_proxy, Some(true));
        assert_eq!(app_settings.proxy_host.as_deref(), Some("127.0.0.1"));
        let loaded = store.load_app_settings().await.expect("load app_settings");
        assert_eq!(loaded.mixed_port, Some(19090));
        assert_eq!(loaded.enable_auto_launch, Some(true));
        assert_eq!(loaded.enable_system_proxy, Some(true));
        assert_eq!(loaded.proxy_host.as_deref(), Some("127.0.0.1"));

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn patch_clash_persists_runtime_base_fields() {
        let root = temp_root("clash-patch");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        store.initialize().await.expect("initialize configs");

        let mut tunnel = serde_yaml_ng::Mapping::new();
        tunnel.insert(
            "network".into(),
            serde_yaml_ng::Value::Sequence(vec!["tcp".into(), "udp".into()]),
        );
        tunnel.insert("address".into(), "127.0.0.1:32097".into());
        tunnel.insert("target".into(), "8.8.8.8:53".into());

        let mut patch = serde_yaml_ng::Mapping::new();
        patch.insert("ipv6".into(), false.into());
        patch.insert(
            "tunnels".into(),
            serde_yaml_ng::Value::Sequence(vec![serde_yaml_ng::Value::Mapping(tunnel)]),
        );

        let clash = store.patch_clash(&patch).await.expect("patch clash");
        assert_eq!(clash.0.get("ipv6"), Some(&serde_yaml_ng::Value::from(false)));

        let saved = crate::yaml::read_mapping(root.join("config.yaml"))
            .await
            .expect("read config");
        assert_eq!(saved.get("ipv6"), Some(&serde_yaml_ng::Value::from(false)));
        assert!(
            saved
                .get("tunnels")
                .and_then(serde_yaml_ng::Value::as_sequence)
                .is_some_and(|items| items.len() == 1)
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn import_and_update_remote_profile_writes_metadata_and_file() -> Result<()> {
        let root = temp_root("remote-profile");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        store.initialize().await?;
        let server = remote_profile_server(2).await?;
        let profiles = store
            .import_remote_profile(&RemoteProfileImport {
                url: server.url.clone(),
                uid: Some("R001".into()),
                name: Some("Remote".into()),
                desc: None,
                option: None,
            })
            .await?;

        assert_eq!(profiles.current.as_deref(), Some("R001"));
        let item = profiles.get_item("R001")?;
        assert_eq!(item.itype.as_deref(), Some("remote"));
        assert_eq!(item.url.as_deref(), Some(server.url.as_str()));
        assert_eq!(item.extra.expect("extra").total, 3);
        assert!(root.join("profiles").join("R001.yaml").is_file());

        let updated = store.update_remote_profile("R001", None).await?;
        assert!(updated.get_item("R001")?.updated.is_some());

        server.done.await??;
        let _ = fs::remove_dir_all(&root);
        Ok(())
    }

    #[tokio::test]
    async fn import_remote_profile_rejects_empty_proxy_sources_without_metadata() -> Result<()> {
        let root = temp_root("remote-profile-empty-import");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        store.initialize().await?;
        let server = remote_profile_server_with_bodies(vec!["proxies: []\nproxy-providers: {}\nrules: []\n"]).await?;

        let error = store
            .import_remote_profile(&RemoteProfileImport {
                url: server.url.clone(),
                uid: Some("Rempty".into()),
                name: Some("Empty".into()),
                desc: None,
                option: None,
            })
            .await
            .expect_err("empty remote profile should fail");

        assert!(error.to_string().contains("proxy-provider entries"));
        assert!(store.load_profiles().await?.get_item("Rempty").is_err());
        assert!(!root.join("profiles").join("Rempty.yaml").exists());

        server.done.await??;
        let _ = fs::remove_dir_all(&root);
        Ok(())
    }

    #[tokio::test]
    async fn update_remote_profile_rejects_empty_sources_without_overwriting_file() -> Result<()> {
        let root = temp_root("remote-profile-empty-update");
        let _ = fs::remove_dir_all(&root);

        let store = ConfigStore::new(AppPaths::from_home(&root));
        store.initialize().await?;
        let valid_body = "proxies:\n  - name: direct\n    type: direct\nrules: []\n";
        let empty_body = "proxies: []\nproxy-providers: {}\nrules: []\n";
        let server = remote_profile_server_with_bodies(vec![valid_body, empty_body]).await?;

        store
            .import_remote_profile(&RemoteProfileImport {
                url: server.url.clone(),
                uid: Some("R001".into()),
                name: Some("Remote".into()),
                desc: None,
                option: None,
            })
            .await?;

        let before = fs::read_to_string(root.join("profiles").join("R001.yaml"))?;
        let error = store
            .update_remote_profile("R001", None)
            .await
            .expect_err("empty update should fail");
        let after = fs::read_to_string(root.join("profiles").join("R001.yaml"))?;

        assert!(error.to_string().contains("proxy-provider entries"));
        assert_eq!(after, before);

        server.done.await??;
        let _ = fs::remove_dir_all(&root);
        Ok(())
    }

    struct RemoteProfileServer {
        url: String,
        done: tokio::task::JoinHandle<Result<()>>,
    }

    async fn remote_profile_server(requests: usize) -> Result<RemoteProfileServer> {
        remote_profile_server_with_bodies(vec![
            "proxies:\n  - name: direct\n    type: direct\nrules: []\n";
            requests
        ])
        .await
    }

    async fn remote_profile_server_with_bodies(bodies: Vec<&'static str>) -> Result<RemoteProfileServer> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let done = tokio::spawn(async move {
            for body in bodies {
                let (mut stream, _) = listener.accept().await?;
                let mut buffer = vec![0; 1024];
                let _ = stream.read(&mut buffer).await?;
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nsubscription-userinfo: upload=1; download=2; total=3; expire=4\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).await?;
            }
            Ok(())
        });

        Ok(RemoteProfileServer {
            url: format!("http://{addr}/sub.yaml"),
            done,
        })
    }

    fn temp_root(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        std::env::temp_dir().join(format!("clash-core-store-{name}-{}-{nanos}", std::process::id()))
    }
}
