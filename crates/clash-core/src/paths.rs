use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::constants::{app, env as env_names, files};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub home_dir: PathBuf,
    pub resources_dir: PathBuf,
    pub profiles_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub clash_config: PathBuf,
    pub settings_config: PathBuf,
    pub profiles_config: PathBuf,
    pub runtime_config: PathBuf,
    pub check_config: PathBuf,
    pub dns_config: PathBuf,
    pub ipc_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppPathSummary {
    pub home_dir: String,
    pub resources_dir: String,
    pub profiles_dir: String,
    pub logs_dir: String,
    pub clash_config: String,
    pub settings_config: String,
    pub profiles_config: String,
    pub runtime_config: String,
    pub check_config: String,
    pub dns_config: String,
    pub ipc_path: String,
}

impl AppPaths {
    #[must_use]
    pub fn from_home(home_dir: impl Into<PathBuf>) -> Self {
        let home_dir = home_dir.into();
        let resources_dir = home_dir.join("resources");
        Self::new(home_dir, resources_dir)
    }

    #[must_use]
    pub fn new(home_dir: impl Into<PathBuf>, resources_dir: impl Into<PathBuf>) -> Self {
        let home_dir = home_dir.into();
        let resources_dir = resources_dir.into();
        let profiles_dir = home_dir.join("profiles");
        let logs_dir = home_dir.join("logs");
        let clash_config = home_dir.join(files::CLASH_CONFIG);
        let settings_config = home_dir.join(files::SETTINGS_CONFIG);
        let profiles_config = home_dir.join(files::PROFILE_YAML);
        let runtime_config = home_dir.join(files::RUNTIME_CONFIG);
        let check_config = home_dir.join(files::CHECK_CONFIG);
        let dns_config = home_dir.join(files::DNS_CONFIG);
        let ipc_path = default_ipc_path(&home_dir);

        Self {
            home_dir,
            resources_dir,
            profiles_dir,
            logs_dir,
            clash_config,
            settings_config,
            profiles_config,
            runtime_config,
            check_config,
            dns_config,
            ipc_path,
        }
    }

    #[must_use]
    pub fn from_default_env() -> Self {
        Self::from_home(default_home_dir())
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        let ipc_dir = self.ipc_path.parent().unwrap_or(&self.home_dir);
        for path in [
            &self.home_dir,
            &self.resources_dir,
            &self.profiles_dir,
            &self.logs_dir,
            ipc_dir,
        ] {
            fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))?;
        }
        Ok(())
    }
}

impl From<&AppPaths> for AppPathSummary {
    fn from(paths: &AppPaths) -> Self {
        Self {
            home_dir: display_path(&paths.home_dir),
            resources_dir: display_path(&paths.resources_dir),
            profiles_dir: display_path(&paths.profiles_dir),
            logs_dir: display_path(&paths.logs_dir),
            clash_config: display_path(&paths.clash_config),
            settings_config: display_path(&paths.settings_config),
            profiles_config: display_path(&paths.profiles_config),
            runtime_config: display_path(&paths.runtime_config),
            check_config: display_path(&paths.check_config),
            dns_config: display_path(&paths.dns_config),
            ipc_path: display_path(&paths.ipc_path),
        }
    }
}

fn display_path(path: &Path) -> String {
    path.as_os_str().to_string_lossy().into_owned()
}

impl Default for AppPaths {
    fn default() -> Self {
        Self::from_home(default_home_dir())
    }
}

#[must_use]
pub fn default_home_dir() -> PathBuf {
    if let Some(value) = env::var_os(env_names::CLASH_TUI_HOME).filter(|value| !value.is_empty()) {
        return PathBuf::from(value);
    }

    if let Some(value) = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(value).join(app::SERVICE_NAME);
    }

    if let Some(value) = env::var_os("HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(value).join(".config").join(app::SERVICE_NAME);
    }

    PathBuf::from(".").join(format!(".{}", app::SERVICE_NAME))
}

#[must_use]
pub fn default_ipc_path(home_dir: &Path) -> PathBuf {
    #[cfg(unix)]
    {
        home_dir.join("ipc").join("mihomo.sock")
    }

    #[cfg(windows)]
    {
        let _ = home_dir;
        PathBuf::from(r"\\.\pipe\clash-tui-mihomo")
    }

    #[cfg(not(any(unix, windows)))]
    {
        home_dir.join("ipc").join("mihomo.sock")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::fs;

    use super::AppPaths;

    #[test]
    fn app_paths_are_derived_from_home() {
        let root = std::env::temp_dir().join(format!("clash-core-paths-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);

        let paths = AppPaths::from_home(&root);
        paths.ensure_dirs().expect("create app dirs");

        assert_eq!(paths.clash_config, root.join("config.yaml"));
        assert_eq!(paths.ipc_path, root.join("ipc").join("mihomo.sock"));
        assert!(root.join("ipc").is_dir());
        assert!(paths.profiles_dir.is_dir());
        assert!(paths.logs_dir.is_dir());

        let _ = fs::remove_dir_all(&root);
    }
}
