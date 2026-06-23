use std::time::Instant;

use gethostname::gethostname;
use network_interface::{NetworkInterface, NetworkInterfaceConfig as _};
use serde::Serialize;
use sysinfo::{Networks, System};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfoPayload {
    pub system_name: String,
    pub system_version: String,
    pub system_kernel_version: String,
    pub system_arch: String,
    pub hostname: String,
    pub app_version: &'static str,
    pub running_mode: &'static str,
    pub is_admin: bool,
    pub uptime_ms: u64,
    pub network_interfaces: Vec<String>,
    pub display: String,
}

pub fn collect(started_at: Instant) -> SystemInfoPayload {
    let system_name = System::name().unwrap_or_else(|| "Unknown".to_owned());
    let system_version = System::long_os_version().unwrap_or_else(|| "Unknown".to_owned());
    let system_kernel_version = System::kernel_version().unwrap_or_else(|| "Unknown".to_owned());
    let system_arch = System::cpu_arch();
    let hostname = system_hostname();
    let is_admin = is_admin();
    let uptime_ms = app_uptime_ms(started_at);
    let network_interfaces = network_interface_names();
    let app_version = env!("CARGO_PKG_VERSION");
    let running_mode = "TUI";
    let display = format_system_info(
        &system_name,
        &system_version,
        &system_kernel_version,
        &system_arch,
        app_version,
        running_mode,
        is_admin,
    );

    SystemInfoPayload {
        system_name,
        system_version,
        system_kernel_version,
        system_arch,
        hostname,
        app_version,
        running_mode,
        is_admin,
        uptime_ms,
        network_interfaces,
        display,
    }
}

pub fn system_hostname() -> String {
    match gethostname().into_string() {
        Ok(name) => name,
        Err(os_string) => {
            let fallback = format!("{os_string:?}");
            fallback.trim_matches('"').to_owned()
        }
    }
}

pub fn network_interface_names() -> Vec<String> {
    let mut networks = Networks::new();
    networks.refresh(false);
    let mut names = networks.keys().map(ToOwned::to_owned).collect::<Vec<_>>();
    names.sort();
    names
}

pub fn network_interfaces_info() -> Result<Vec<NetworkInterface>, network_interface::Error> {
    let names = network_interface_names();
    let interfaces = NetworkInterface::show()?;

    if names.is_empty() {
        return Ok(interfaces);
    }

    Ok(interfaces
        .into_iter()
        .filter(|interface| names.contains(&interface.name))
        .collect())
}

pub fn is_admin() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions and does not dereference pointers.
        unsafe { libc::geteuid() == 0 }
    }

    #[cfg(not(unix))]
    {
        false
    }
}

pub fn app_uptime_ms(started_at: Instant) -> u64 {
    match u64::try_from(started_at.elapsed().as_millis()) {
        Ok(value) => value,
        Err(_) => u64::MAX,
    }
}

fn format_system_info(
    system_name: &str,
    system_version: &str,
    system_kernel_version: &str,
    system_arch: &str,
    app_version: &str,
    running_mode: &str,
    is_admin: bool,
) -> String {
    format!(
        "System Name: {system_name}\nSystem Version: {system_version}\nSystem kernel Version: {system_kernel_version}\nSystem Arch: {system_arch}\nClash TUI Version: {app_version}\nRunning Mode: {running_mode}\nIs Admin: {is_admin}"
    )
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::{collect, format_system_info};

    #[test]
    fn display_matches_desktop_system_info_shape() {
        let display = format_system_info("Linux", "Linux 6.0", "6.0", "x86_64", "2.5.1", "TUI", false);

        assert!(display.contains("System Name: Linux"));
        assert!(display.contains("System Version: Linux 6.0"));
        assert!(display.contains("System kernel Version: 6.0"));
        assert!(display.contains("Running Mode: TUI"));
        assert!(display.contains("Is Admin: false"));
    }

    #[test]
    fn collect_includes_display_and_nonzero_shape() {
        let payload = collect(Instant::now());

        assert_eq!(payload.running_mode, "TUI");
        assert!(payload.display.contains("System Name:"));
        assert!(payload.display.contains("Clash TUI Version:"));
    }
}
