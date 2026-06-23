use std::{
    fmt, fs,
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};
use clash_core::AppPaths;

pub const ENV_HOME_DIR: &str = "CLASH_TUI_HOME";
pub const ENV_RESOURCE_DIR: &str = "CLASH_TUI_RESOURCE_DIR";
pub const ENV_MIHOMO_BIN: &str = "CLASH_TUI_MIHOMO_BIN";
pub const ENV_SUBSCRIPTION_CHECK_INTERVAL_SECS: &str = "CLASH_TUI_SUBSCRIPTION_CHECK_INTERVAL_SECS";
pub const DEFAULT_SUBSCRIPTION_CHECK_INTERVAL_SECS: u64 = 300;
const INSTALL_LAYOUT_FILE: &str = "install-layout.env";

#[derive(Clone, PartialEq, Eq)]
pub struct ClashTuiOptions {
    pub home_dir: Option<PathBuf>,
    pub resource_dir: Option<PathBuf>,
    pub mihomo_bin: Option<PathBuf>,
    pub subscription_check_interval_secs: u64,
}

impl ClashTuiOptions {
    pub fn new(
        home_dir: Option<PathBuf>,
        resource_dir: Option<PathBuf>,
        mihomo_bin: Option<PathBuf>,
        subscription_check_interval_secs: u64,
    ) -> Result<Self> {
        if subscription_check_interval_secs == 0 {
            bail!("--subscription-check-interval-secs must be greater than 0");
        }

        Ok(Self {
            home_dir,
            resource_dir,
            mihomo_bin,
            subscription_check_interval_secs,
        })
    }

    pub fn app_paths(&self) -> AppPaths {
        let defaults = installed_layout_defaults();
        self.app_paths_with_defaults(defaults.as_ref())
    }

    fn app_paths_with_defaults(&self, defaults: Option<&InstallLayoutDefaults>) -> AppPaths {
        let home_dir = self
            .home_dir
            .clone()
            .or_else(|| defaults.and_then(|layout| layout.home_dir.clone()))
            .unwrap_or_else(clash_core::paths::default_home_dir);
        let resource_dir = self
            .resource_dir
            .clone()
            .or_else(|| defaults.and_then(|layout| layout.resource_dir.clone()))
            .unwrap_or_else(|| home_dir.join("resources"));
        AppPaths::new(home_dir, resource_dir)
    }

    #[must_use]
    pub fn resolved_mihomo_bin(&self, paths: &AppPaths) -> PathBuf {
        let defaults = installed_layout_defaults();
        self.resolved_mihomo_bin_with_defaults(paths, defaults.as_ref())
    }

    fn resolved_mihomo_bin_with_defaults(&self, paths: &AppPaths, defaults: Option<&InstallLayoutDefaults>) -> PathBuf {
        self.mihomo_bin
            .clone()
            .or_else(|| defaults.and_then(|layout| layout.mihomo_bin.clone()))
            .unwrap_or_else(|| paths.resources_dir.join("mihomo"))
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct InstallLayoutDefaults {
    home_dir: Option<PathBuf>,
    resource_dir: Option<PathBuf>,
    mihomo_bin: Option<PathBuf>,
}

fn installed_layout_defaults() -> Option<InstallLayoutDefaults> {
    let exe = std::env::current_exe().ok()?;
    let exe = fs::canonicalize(&exe).unwrap_or(exe);
    installed_layout_defaults_from_exe(&exe)
}

fn installed_layout_defaults_from_exe(exe: &Path) -> Option<InstallLayoutDefaults> {
    let bin_dir = exe.parent()?;
    let layout_file = bin_dir.join(INSTALL_LAYOUT_FILE);
    let mut defaults = fs::read_to_string(layout_file)
        .ok()
        .map(|content| parse_install_layout(&content))
        .unwrap_or_default();

    let bundled_mihomo = bin_dir.join("resources").join("mihomo");
    if defaults.resource_dir.is_none() && bundled_mihomo.is_file() {
        defaults.resource_dir = Some(bin_dir.join("resources"));
    }
    if defaults.mihomo_bin.is_none() && bundled_mihomo.is_file() {
        defaults.mihomo_bin = Some(bundled_mihomo);
    }
    if defaults.home_dir.is_none() {
        defaults.home_dir = infer_opt_home_dir(bin_dir);
    }

    if defaults.home_dir.is_none() && defaults.resource_dir.is_none() && defaults.mihomo_bin.is_none() {
        None
    } else {
        Some(defaults)
    }
}

fn infer_opt_home_dir(bin_dir: &Path) -> Option<PathBuf> {
    if bin_dir.parent()? != Path::new("/opt") {
        return None;
    }
    Some(PathBuf::from("/var/lib").join(bin_dir.file_name()?))
}

fn parse_install_layout(content: &str) -> InstallLayoutDefaults {
    let mut defaults = InstallLayoutDefaults::default();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let path = PathBuf::from(value);
        match key.trim() {
            ENV_HOME_DIR => defaults.home_dir = Some(path),
            ENV_RESOURCE_DIR => defaults.resource_dir = Some(path),
            ENV_MIHOMO_BIN => defaults.mihomo_bin = Some(path),
            _ => {}
        }
    }
    defaults
}

impl fmt::Debug for ClashTuiOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClashTuiOptions")
            .field("home_dir", &self.home_dir)
            .field("resource_dir", &self.resource_dir)
            .field("mihomo_bin", &self.mihomo_bin)
            .field(
                "subscription_check_interval_secs",
                &self.subscription_check_interval_secs,
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::{fs, path::PathBuf};

    use super::{
        ClashTuiOptions, DEFAULT_SUBSCRIPTION_CHECK_INTERVAL_SECS, ENV_HOME_DIR, ENV_MIHOMO_BIN, ENV_RESOURCE_DIR,
        infer_opt_home_dir, installed_layout_defaults_from_exe, parse_install_layout,
    };

    #[test]
    fn options_reject_zero_subscription_interval() {
        let error = ClashTuiOptions::new(None, None, None, 0).expect_err("zero interval");

        assert!(error.to_string().contains("greater than 0"));
    }

    #[test]
    fn options_build_default_paths() {
        let options =
            ClashTuiOptions::new(None, None, None, DEFAULT_SUBSCRIPTION_CHECK_INTERVAL_SECS).expect("options");
        let paths = options.app_paths();

        assert!(paths.resources_dir.ends_with("resources"));
    }

    #[test]
    fn install_layout_file_is_used_as_defaults() {
        let root = std::env::temp_dir().join(format!("clash-tui-layout-{}", std::process::id()));
        let prefix = root.join("opt").join("clash-tui");
        let resources = prefix.join("resources");
        fs::create_dir_all(&resources).expect("resources");
        fs::write(resources.join("mihomo"), "").expect("mihomo");
        fs::write(
            prefix.join("install-layout.env"),
            format!(
                "{ENV_HOME_DIR}={}\n{ENV_RESOURCE_DIR}={}\n{ENV_MIHOMO_BIN}={}\n",
                root.join("state").display(),
                resources.display(),
                resources.join("mihomo").display()
            ),
        )
        .expect("layout");

        let defaults = installed_layout_defaults_from_exe(&prefix.join("clash-tui")).expect("defaults");
        let options =
            ClashTuiOptions::new(None, None, None, DEFAULT_SUBSCRIPTION_CHECK_INTERVAL_SECS).expect("options");
        let paths = options.app_paths_with_defaults(Some(&defaults));

        assert_eq!(paths.home_dir, root.join("state"));
        assert_eq!(paths.resources_dir, resources);
        assert_eq!(
            options.resolved_mihomo_bin_with_defaults(&paths, Some(&defaults)),
            resources.join("mihomo")
        );

        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn explicit_options_override_install_layout_defaults() {
        let defaults = parse_install_layout(&format!(
            "{ENV_HOME_DIR}=/var/lib/from-layout\n{ENV_RESOURCE_DIR}=/opt/from-layout/resources\n{ENV_MIHOMO_BIN}=/opt/from-layout/resources/mihomo\n"
        ));
        let options = ClashTuiOptions::new(
            Some(PathBuf::from("/tmp/home")),
            Some(PathBuf::from("/tmp/resources")),
            Some(PathBuf::from("/tmp/mihomo")),
            DEFAULT_SUBSCRIPTION_CHECK_INTERVAL_SECS,
        )
        .expect("options");
        let paths = options.app_paths_with_defaults(Some(&defaults));

        assert_eq!(paths.home_dir, PathBuf::from("/tmp/home"));
        assert_eq!(paths.resources_dir, PathBuf::from("/tmp/resources"));
        assert_eq!(
            options.resolved_mihomo_bin_with_defaults(&paths, Some(&defaults)),
            PathBuf::from("/tmp/mihomo")
        );
    }

    #[test]
    fn opt_install_home_dir_is_inferred_from_prefix_name() {
        assert_eq!(
            infer_opt_home_dir(std::path::Path::new("/opt/clash-tui")),
            Some(PathBuf::from("/var/lib/clash-tui"))
        );
        assert_eq!(infer_opt_home_dir(std::path::Path::new("/tmp/clash-tui")), None);
    }
}
