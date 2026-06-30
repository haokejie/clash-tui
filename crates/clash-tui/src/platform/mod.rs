use std::path::Path;

use crate::actions::system::SwitchStatus;
use clash_core::{AppSettings, constants::network};
use serde::Serialize;

const DEFAULT_SYSTEM_PROXY_BYPASS: &str = "localhost,127.0.0.1,192.168.0.0/16,10.0.0.0/8,172.16.0.0/12,::1";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemProxyDiagnostics {
    pub platform: String,
    pub endpoint: SystemProxyEndpoint,
    pub auto_apply_supported: bool,
    pub can_auto_apply: bool,
    pub checks: Vec<SystemProxyCheck>,
    pub manual_action: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunDiagnostics {
    pub platform: String,
    pub enabled: bool,
    pub can_enable: bool,
    pub checks: Vec<TunCheck>,
    pub manual_action: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunCheck {
    pub name: String,
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemProxyEndpoint {
    pub host: String,
    pub port: u16,
    pub bypass: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemProxyCheck {
    pub name: String,
    pub ok: bool,
    pub message: String,
}

pub fn tun_status(enabled: bool) -> SwitchStatus {
    SwitchStatus {
        enabled,
        platform: platform_name().into(),
        config_saved: true,
        runtime_generated: false,
        runtime_applied: None,
        platform_applied: None,
        requires_core_restart: false,
        core_restarted: false,
        core_state: None,
        runtime_path: None,
        manual_action: if enabled {
            Some("确认 mihomo 具备创建 TUN 设备的权限；Linux 通常需要 root 或 CAP_NET_ADMIN".into())
        } else {
            None
        },
        message: if enabled {
            "TUN 已在本地配置中开启；实际接管由 mihomo runtime 执行".into()
        } else {
            "TUN 已在本地配置中关闭".into()
        },
    }
}

pub fn tun_diagnostics(enabled: bool, mihomo_bin: &Path) -> TunDiagnostics {
    #[cfg(not(target_os = "linux"))]
    let _ = mihomo_bin;

    #[cfg(target_os = "linux")]
    {
        linux_tun_diagnostics(enabled, mihomo_bin)
    }

    #[cfg(not(target_os = "linux"))]
    {
        let platform = platform_name().to_owned();
        TunDiagnostics {
            platform: platform.clone(),
            enabled,
            can_enable: false,
            checks: vec![TunCheck {
                name: "platform-adapter".into(),
                ok: false,
                message: format!("当前构建尚未实现 {platform} 的 TUN 环境诊断"),
            }],
            manual_action: Some(
                "请确认当前平台的 mihomo TUN 权限和系统网络扩展要求；开启失败时执行 tun off 和 core stop 恢复".into(),
            ),
            message: "当前平台只能保存 TUN 配置，是否能接管网络需以 mihomo 启动结果为准".into(),
        }
    }
}

pub fn system_proxy_status(enabled: bool) -> SwitchStatus {
    SwitchStatus {
        enabled,
        platform: platform_name().into(),
        config_saved: true,
        runtime_generated: false,
        runtime_applied: None,
        platform_applied: None,
        requires_core_restart: false,
        core_restarted: false,
        core_state: None,
        runtime_path: None,
        manual_action: if enabled {
            Some("执行 system-proxy on/off 在当前平台应用已保存的配置".into())
        } else {
            None
        },
        message: if enabled {
            "系统代理标记已开启；是否真正应用以平台适配器结果为准".into()
        } else {
            "系统代理标记已关闭".into()
        },
    }
}

pub fn system_proxy_diagnostics(app_settings: &AppSettings) -> SystemProxyDiagnostics {
    let (host, port, bypass) = system_proxy_endpoint(app_settings);
    let endpoint = SystemProxyEndpoint { host, port, bypass };

    #[cfg(target_os = "linux")]
    {
        linux_system_proxy_diagnostics(endpoint)
    }

    #[cfg(not(target_os = "linux"))]
    {
        let platform = platform_name().to_owned();
        let manual_action = Some(format!(
            "请在 {platform} 系统代理中手动设置 HTTP/HTTPS/SOCKS 主机 {}、端口 {}，忽略主机 {}",
            endpoint.host, endpoint.port, endpoint.bypass
        ));
        SystemProxyDiagnostics {
            platform,
            endpoint,
            auto_apply_supported: false,
            can_auto_apply: false,
            checks: vec![SystemProxyCheck {
                name: "platform-adapter".into(),
                ok: false,
                message: "当前构建尚未实现该平台的系统代理自动应用".into(),
            }],
            manual_action,
            message: "当前平台只能手动配置系统代理".into(),
        }
    }
}

pub fn apply_system_proxy(app_settings: &AppSettings, enabled: bool) -> SwitchStatus {
    #[cfg(target_os = "linux")]
    {
        linux_apply_system_proxy(app_settings, enabled)
    }

    #[cfg(not(target_os = "linux"))]
    {
        let mut status = system_proxy_status(enabled);
        if enabled {
            status.message = format!("系统代理标记已开启；当前构建尚未实现 {} 的自动应用", status.platform);
            status.platform_applied = Some(false);
            let (host, port, bypass) = system_proxy_endpoint(app_settings);
            status.manual_action = Some(format!(
                "请在 {} 系统代理中手动设置 HTTP/HTTPS/SOCKS 主机 {}、端口 {}，忽略主机 {}",
                status.platform, host, port, bypass
            ));
        }
        status
    }
}

const fn platform_name() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "linux"
    }

    #[cfg(target_os = "macos")]
    {
        "macos"
    }

    #[cfg(target_os = "windows")]
    {
        "windows"
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        "unknown"
    }
}

#[cfg(target_os = "linux")]
fn linux_apply_system_proxy(app_settings: &AppSettings, enabled: bool) -> SwitchStatus {
    let mut status = system_proxy_status(enabled);
    let (host, port, bypass) = system_proxy_endpoint(app_settings);
    let diagnostics = linux_system_proxy_diagnostics(SystemProxyEndpoint {
        host: host.clone(),
        port,
        bypass: bypass.clone(),
    });
    if !diagnostics.can_auto_apply {
        status.message = format!("系统代理配置已保存，但 Linux 平台应用失败：{}", diagnostics.message);
        status.platform_applied = Some(false);
        status.manual_action = Some(if enabled {
            diagnostics
                .manual_action
                .unwrap_or_else(|| linux_enable_manual_action(&host, port, &bypass, &diagnostics.message))
        } else {
            linux_disable_manual_action(&diagnostics.message)
        });
        return status;
    }

    let result = if enabled {
        linux_enable_gsettings_proxy(&host, port, &bypass)
    } else {
        linux_disable_gsettings_proxy()
    };

    match result {
        Ok(message) => {
            status.message = message;
            status.platform_applied = Some(true);
            status.manual_action = None;
        }
        Err(message) => {
            status.message = format!("系统代理配置已保存，但 Linux 平台应用失败：{message}");
            status.platform_applied = Some(false);
            status.manual_action = Some(if enabled {
                linux_enable_manual_action(&host, port, &bypass, &message)
            } else {
                linux_disable_manual_action(&message)
            });
        }
    }
    status
}

fn system_proxy_endpoint(app_settings: &AppSettings) -> (String, u16, String) {
    let host = app_settings.proxy_host.as_deref().unwrap_or("127.0.0.1").to_owned();
    let port = app_settings.mixed_port.unwrap_or(network::ports::DEFAULT_MIXED);
    let bypass = app_settings
        .system_proxy_bypass
        .as_deref()
        .unwrap_or(DEFAULT_SYSTEM_PROXY_BYPASS)
        .to_owned();
    (host, port, bypass)
}

#[cfg(target_os = "linux")]
fn linux_system_proxy_diagnostics(endpoint: SystemProxyEndpoint) -> SystemProxyDiagnostics {
    let gsettings_exists = linux_command_exists("gsettings");
    let schema_result = gsettings_exists
        .then(linux_require_gsettings_proxy_schema)
        .unwrap_or_else(|| Err("未找到 gsettings；请安装桌面设置后端，或手动配置系统代理".into()));
    let session_check = linux_desktop_session_check();
    let session_ok = session_check.ok;
    let schema_message = schema_result
        .as_ref()
        .map(|_| "已找到 org.gnome.system.proxy schema".to_owned())
        .unwrap_or_else(|message| message.clone());
    let checks = vec![
        SystemProxyCheck {
            name: "gsettings".into(),
            ok: gsettings_exists,
            message: if gsettings_exists {
                "已找到 gsettings".into()
            } else {
                "未找到 gsettings".into()
            },
        },
        SystemProxyCheck {
            name: "org.gnome.system.proxy".into(),
            ok: schema_result.is_ok(),
            message: schema_message.clone(),
        },
        SystemProxyCheck {
            name: "desktop-session".into(),
            ok: session_check.ok,
            message: session_check.message,
        },
    ];
    let can_auto_apply = linux_system_proxy_can_auto_apply(gsettings_exists, schema_result.is_ok(), session_ok);
    let failure_message = checks
        .iter()
        .find(|check| !check.ok)
        .map(|check| check.message.as_str())
        .unwrap_or(schema_message.as_str());

    let manual_action = (!can_auto_apply)
        .then(|| linux_enable_manual_action(&endpoint.host, endpoint.port, &endpoint.bypass, failure_message));
    let message = if can_auto_apply {
        format!(
            "当前 Linux 环境具备 GNOME gsettings 自动应用条件；将设置 HTTP/HTTPS/SOCKS {}:{}",
            endpoint.host, endpoint.port
        )
    } else {
        format!("当前 Linux 环境无法自动应用系统代理：{failure_message}")
    };

    SystemProxyDiagnostics {
        platform: platform_name().into(),
        endpoint,
        auto_apply_supported: true,
        can_auto_apply,
        checks,
        manual_action,
        message,
    }
}

#[cfg(any(target_os = "linux", test))]
const fn linux_system_proxy_can_auto_apply(gsettings_exists: bool, schema_ok: bool, desktop_session_ok: bool) -> bool {
    gsettings_exists && schema_ok && desktop_session_ok
}

#[cfg(target_os = "linux")]
fn linux_tun_diagnostics(enabled: bool, mihomo_bin: &Path) -> TunDiagnostics {
    let effective_uid = linux_effective_uid();
    let getcap_exists = linux_command_exists("getcap");
    let setcap_exists = linux_command_exists("setcap");
    let tun_device = linux_tun_device_check();
    let privilege = linux_tun_privilege_check(mihomo_bin, effective_uid, getcap_exists);
    let capability_tools = linux_tun_capability_tools_check(effective_uid, getcap_exists, setcap_exists);
    let ip_command_exists = linux_command_exists("ip");
    let checks = vec![
        tun_device.clone(),
        privilege.clone(),
        capability_tools,
        TunCheck {
            name: "iproute2".into(),
            ok: ip_command_exists,
            message: if ip_command_exists {
                "已找到 ip 命令，可用于验证 Meta 网卡和路由".into()
            } else {
                "未找到 ip 命令；不一定阻止 mihomo 创建 TUN，但会影响本机诊断验证".into()
            },
        },
    ];
    let can_enable = tun_device.ok && privilege.ok;
    let manual_action =
        (!can_enable).then(|| linux_tun_manual_action(&tun_device, &privilege, getcap_exists, setcap_exists));
    let message = if can_enable {
        "当前 Linux 环境具备 TUN 基本条件；开启后会写入 runtime，Core 启动时创建 Meta 设备".into()
    } else {
        let reason = checks
            .iter()
            .find(|check| !check.ok)
            .map(|check| check.message.as_str())
            .unwrap_or("未知 TUN 环境问题");
        format!("当前 Linux 环境不满足 TUN 开启条件：{reason}")
    };

    TunDiagnostics {
        platform: platform_name().into(),
        enabled,
        can_enable,
        checks,
        manual_action,
        message,
    }
}

#[cfg(target_os = "linux")]
fn linux_tun_device_check() -> TunCheck {
    use std::os::unix::fs::FileTypeExt as _;

    let path = Path::new("/dev/net/tun");
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.file_type().is_char_device() => TunCheck {
            name: "dev-net-tun".into(),
            ok: true,
            message: "/dev/net/tun 存在".into(),
        },
        Ok(_) => TunCheck {
            name: "dev-net-tun".into(),
            ok: false,
            message: "/dev/net/tun 存在但不是字符设备".into(),
        },
        Err(err) => TunCheck {
            name: "dev-net-tun".into(),
            ok: false,
            message: format!("未找到 /dev/net/tun：{err}"),
        },
    }
}

#[cfg(target_os = "linux")]
fn linux_tun_privilege_check(mihomo_bin: &Path, effective_uid: u32, getcap_exists: bool) -> TunCheck {
    if effective_uid == 0 {
        return TunCheck {
            name: "privilege".into(),
            ok: true,
            message: "当前进程为 root，具备创建 TUN 所需的基础权限".into(),
        };
    }

    if linux_mihomo_has_cap_net_admin(mihomo_bin, getcap_exists) {
        return TunCheck {
            name: "privilege".into(),
            ok: true,
            message: "当前 mihomo 二进制已授予 CAP_NET_ADMIN".into(),
        };
    }

    let message = if !mihomo_bin.is_file() {
        "当前进程不是 root，且 mihomo 二进制不存在或不可读，无法检测 CAP_NET_ADMIN"
    } else if !getcap_exists {
        "当前进程不是 root，且未找到 getcap，无法检测 mihomo CAP_NET_ADMIN"
    } else {
        "当前进程不是 root，且 mihomo 未授予 CAP_NET_ADMIN"
    };

    TunCheck {
        name: "privilege".into(),
        ok: false,
        message: message.into(),
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_tun_capability_tools_check(effective_uid: u32, getcap_exists: bool, setcap_exists: bool) -> TunCheck {
    let is_root = effective_uid == 0;
    let ok = is_root || getcap_exists;
    let message = match (is_root, getcap_exists, setcap_exists) {
        (true, true, true) => "当前进程为 root；也已找到 getcap/setcap，可用于非 root 场景排查",
        (true, true, false) => "当前进程为 root；已找到 getcap，未找到 setcap，非 root 授权需安装 libcap 工具",
        (true, false, true) => "当前进程为 root；未找到 getcap，已找到 setcap，非 root CAP 状态检测能力受限",
        (true, false, false) => "当前进程为 root；未找到 getcap/setcap，非 root 授权和检测需安装 libcap 工具",
        (false, true, true) => "已找到 getcap/setcap，可检测并授予 mihomo CAP_NET_ADMIN",
        (false, true, false) => {
            "已找到 getcap，可检测 mihomo CAP_NET_ADMIN；未找到 setcap，如需授权请安装 libcap 工具或用 root 执行 setcap"
        }
        (false, false, true) => {
            "未找到 getcap，无法检测 mihomo CAP_NET_ADMIN；已找到 setcap，可授权但需重新运行 doctor 确认"
        }
        (false, false, false) => "未找到 getcap/setcap；非 root 环境无法检测或授予 mihomo CAP_NET_ADMIN",
    };
    TunCheck {
        name: "capability-tools".into(),
        ok,
        message: message.into(),
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_tun_manual_action(
    tun_device: &TunCheck,
    privilege: &TunCheck,
    getcap_exists: bool,
    setcap_exists: bool,
) -> String {
    let mut action = "请确认 /dev/net/tun 存在，并以 root 运行或为 mihomo 授予 CAP_NET_ADMIN".to_owned();
    if !tun_device.ok {
        action.push_str("；缺少 /dev/net/tun 时请加载 tun 模块，或在容器/宿主机中挂载 TUN 设备");
    }
    if !privilege.ok {
        action.push_str("；非 root 可使用 setcap cap_net_admin=+ep <mihomo> 授权，或改用 root 运行");
    }
    if !getcap_exists || !setcap_exists {
        action.push_str("；请安装提供 getcap/setcap 的 libcap 工具后重试 tun doctor");
    }
    action.push_str("；开启失败时执行 tun off 和 core stop 恢复");
    action
}

#[cfg(target_os = "linux")]
fn linux_effective_uid() -> u32 {
    // SAFETY: geteuid is a side-effect-free libc call.
    unsafe { libc::geteuid() }
}

#[cfg(target_os = "linux")]
fn linux_mihomo_has_cap_net_admin(mihomo_bin: &Path, getcap_exists: bool) -> bool {
    if !mihomo_bin.is_file() || !getcap_exists {
        return false;
    }
    let output = std::process::Command::new("getcap").arg(mihomo_bin).output();
    let Ok(output) = output else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    linux_getcap_has_cap_net_admin(&stdout)
}

#[cfg(target_os = "linux")]
fn linux_getcap_has_cap_net_admin(stdout: &str) -> bool {
    stdout
        .lines()
        .flat_map(|line| line.split_whitespace().skip(1))
        .any(linux_capability_set_has_net_admin)
}

#[cfg(target_os = "linux")]
fn linux_capability_set_has_net_admin(capability_set: &str) -> bool {
    let Some((capabilities, flags)) = capability_set
        .split_once('=')
        .or_else(|| capability_set.split_once('+'))
    else {
        return false;
    };
    let has_effective_and_permitted = flags.contains('e') && flags.contains('p');
    has_effective_and_permitted
        && capabilities
            .split(',')
            .any(|capability| capability.trim() == "cap_net_admin")
}

#[cfg(target_os = "linux")]
fn linux_enable_gsettings_proxy(host: &str, port: u16, bypass: &str) -> Result<String, String> {
    linux_require_gsettings_proxy_schema()?;
    linux_gsettings(&["set", "org.gnome.system.proxy", "mode", "manual"])?;
    linux_gsettings(&["set", "org.gnome.system.proxy.http", "host", host])?;
    linux_gsettings(&["set", "org.gnome.system.proxy.http", "port", &port.to_string()])?;
    linux_gsettings(&["set", "org.gnome.system.proxy.https", "host", host])?;
    linux_gsettings(&["set", "org.gnome.system.proxy.https", "port", &port.to_string()])?;
    linux_gsettings(&["set", "org.gnome.system.proxy.socks", "host", host])?;
    linux_gsettings(&["set", "org.gnome.system.proxy.socks", "port", &port.to_string()])?;
    linux_gsettings(&[
        "set",
        "org.gnome.system.proxy",
        "ignore-hosts",
        &linux_gsettings_string_array(bypass),
    ])?;
    Ok(format!("已通过 gsettings 应用系统代理 {host}:{port}"))
}

#[cfg(target_os = "linux")]
fn linux_disable_gsettings_proxy() -> Result<String, String> {
    linux_require_gsettings_proxy_schema()?;
    linux_gsettings(&["set", "org.gnome.system.proxy", "mode", "none"])?;
    Ok("已通过 gsettings 将系统代理恢复为 mode=none".into())
}

#[cfg(any(target_os = "linux", test))]
fn linux_enable_manual_action(host: &str, port: u16, bypass: &str, reason: &str) -> String {
    format!(
        "{reason}；可手动在桌面系统代理中设置 HTTP/HTTPS/SOCKS 主机 {host}、端口 {port}，忽略主机 {bypass}；GNOME 可先确认 org.gnome.system.proxy schema 后执行：gsettings set org.gnome.system.proxy mode manual"
    )
}

#[cfg(target_os = "linux")]
fn linux_disable_manual_action(reason: &str) -> String {
    format!("{reason}；可手动在桌面系统设置中关闭代理；GNOME 可执行：gsettings set org.gnome.system.proxy mode none")
}

#[cfg(target_os = "linux")]
fn linux_require_gsettings_proxy_schema() -> Result<(), String> {
    if !linux_command_exists("gsettings") {
        return Err("未找到 gsettings；请安装桌面设置后端，或手动配置系统代理".into());
    }
    let output = std::process::Command::new("gsettings")
        .arg("list-schemas")
        .output()
        .map_err(|err| format!("failed to run gsettings list-schemas: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gsettings list-schemas failed: {}", stderr.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.lines().any(|line| line == "org.gnome.system.proxy") {
        return Ok(());
    }
    Err("gsettings 缺少 org.gnome.system.proxy schema；当前环境可能不是 GNOME/桌面会话，请手动配置系统代理".into())
}

#[cfg(target_os = "linux")]
fn linux_gsettings(args: &[&str]) -> Result<(), String> {
    let output = std::process::Command::new("gsettings")
        .args(args)
        .output()
        .map_err(|err| format!("failed to run gsettings: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(stderr.trim().to_owned())
}

#[cfg(target_os = "linux")]
fn linux_command_exists(command: &str) -> bool {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {command} >/dev/null 2>&1"))
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(target_os = "linux")]
fn linux_desktop_session_check() -> SystemProxyCheck {
    linux_desktop_session_check_from_values(
        std::env::var("DBUS_SESSION_BUS_ADDRESS").ok().as_deref(),
        std::env::var("WAYLAND_DISPLAY").ok().as_deref(),
        std::env::var("DISPLAY").ok().as_deref(),
        std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref(),
        std::env::var("DESKTOP_SESSION").ok().as_deref(),
    )
}

#[cfg(any(target_os = "linux", test))]
fn linux_desktop_session_check_from_values(
    dbus: Option<&str>,
    wayland: Option<&str>,
    display: Option<&str>,
    xdg_current_desktop: Option<&str>,
    desktop_session: Option<&str>,
) -> SystemProxyCheck {
    let dbus = dbus.map(str::trim).filter(|value| !value.is_empty());
    let mut hints = Vec::new();
    for (key, value) in [
        ("WAYLAND_DISPLAY", wayland),
        ("DISPLAY", display),
        ("XDG_CURRENT_DESKTOP", xdg_current_desktop),
        ("DESKTOP_SESSION", desktop_session),
    ] {
        if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
            hints.push(format!("{key}={value}"));
        }
    }

    let has_desktop_marker = !hints.is_empty();
    let message = match (dbus, has_desktop_marker) {
        (Some(dbus), true) => {
            let mut parts = vec![format!("DBUS_SESSION_BUS_ADDRESS={dbus}")];
            parts.extend(hints.iter().cloned());
            parts.join(", ")
        }
        (Some(dbus), false) => {
            format!(
                "已检测到 DBUS_SESSION_BUS_ADDRESS={dbus}，但未检测到 DISPLAY/WAYLAND_DISPLAY/XDG_CURRENT_DESKTOP/DESKTOP_SESSION；可能不是已登录桌面会话"
            )
        }
        (None, true) => {
            format!(
                "未检测到 DBUS_SESSION_BUS_ADDRESS；服务器、sudo/root 或 SSH 会话可能无法写入桌面代理；已检测到 {}",
                hints.join(", ")
            )
        }
        (None, false) => {
            "未检测到 DBUS_SESSION_BUS_ADDRESS/DISPLAY/WAYLAND_DISPLAY/XDG_CURRENT_DESKTOP/DESKTOP_SESSION；服务器或 root 会话可能无法写入桌面代理"
                .into()
        }
    };

    SystemProxyCheck {
        name: "desktop-session".into(),
        ok: dbus.is_some() && has_desktop_marker,
        message,
    }
}

#[cfg(target_os = "linux")]
fn linux_gsettings_string_array(value: &str) -> String {
    let entries = value
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| format!("'{}'", entry.replace('\'', "\\'")))
        .collect::<Vec<_>>();
    format!("[{}]", entries.join(", "))
}

#[cfg(test)]
mod tests {
    #[test]
    fn linux_enable_manual_action_is_actionable_without_url_scheme() {
        let message = super::linux_enable_manual_action(
            "127.0.0.1",
            7897,
            "localhost,127.0.0.1",
            "gsettings 缺少 org.gnome.system.proxy schema",
        );

        assert!(message.contains("HTTP/HTTPS/SOCKS"));
        assert!(message.contains("主机 127.0.0.1"));
        assert!(message.contains("端口 7897"));
        assert!(message.contains("忽略主机 localhost,127.0.0.1"));
        assert!(message.contains("gsettings set org.gnome.system.proxy mode manual"));
        assert!(!message.contains("http://"));
        assert!(!message.contains("https://"));
    }

    #[test]
    fn linux_system_proxy_auto_apply_requires_desktop_session() {
        assert!(super::linux_system_proxy_can_auto_apply(true, true, true));
        assert!(!super::linux_system_proxy_can_auto_apply(true, true, false));
        assert!(!super::linux_system_proxy_can_auto_apply(true, false, true));
        assert!(!super::linux_system_proxy_can_auto_apply(false, true, true));
    }

    #[test]
    fn linux_desktop_session_check_requires_dbus_and_desktop_marker() {
        let only_desktop = super::linux_desktop_session_check_from_values(None, None, None, Some("GNOME"), None);
        assert_eq!(only_desktop.name, "desktop-session");
        assert!(!only_desktop.ok);
        assert!(only_desktop.message.contains("未检测到 DBUS_SESSION_BUS_ADDRESS"));
        assert!(only_desktop.message.contains("XDG_CURRENT_DESKTOP=GNOME"));

        let only_dbus = super::linux_desktop_session_check_from_values(
            Some("unix:path=/run/user/1000/bus"),
            None,
            None,
            None,
            None,
        );
        assert!(!only_dbus.ok);
        assert!(only_dbus.message.contains("已检测到 DBUS_SESSION_BUS_ADDRESS"));
        assert!(only_dbus.message.contains("未检测到 DISPLAY/WAYLAND_DISPLAY"));

        let gnome_wayland = super::linux_desktop_session_check_from_values(
            Some("unix:path=/run/user/1000/bus"),
            Some("wayland-0"),
            None,
            Some("GNOME"),
            Some("gnome"),
        );
        assert!(gnome_wayland.ok);
        assert!(
            gnome_wayland
                .message
                .contains("DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus")
        );
        assert!(gnome_wayland.message.contains("WAYLAND_DISPLAY=wayland-0"));
        assert!(gnome_wayland.message.contains("XDG_CURRENT_DESKTOP=GNOME"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_system_proxy_diagnostics_reports_endpoint_without_url_scheme() {
        let app_settings = clash_core::AppSettings {
            proxy_host: Some("127.0.0.1".into()),
            mixed_port: Some(7897),
            system_proxy_bypass: Some("localhost,127.0.0.1".into()),
            ..Default::default()
        };

        let diagnostics = super::system_proxy_diagnostics(&app_settings);

        assert_eq!(diagnostics.endpoint.host, "127.0.0.1");
        assert_eq!(diagnostics.endpoint.port, 7897);
        assert!(diagnostics.checks.iter().any(|check| check.name == "gsettings"));
        assert!(!diagnostics.message.contains("http://"));
        assert!(!diagnostics.message.contains("https://"));
        if let Some(manual_action) = diagnostics.manual_action {
            assert!(!manual_action.contains("http://"));
            assert!(!manual_action.contains("https://"));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_tun_diagnostics_is_readable_and_does_not_leak_urls() {
        let diagnostics = super::tun_diagnostics(false, std::path::Path::new("/missing/mihomo"));

        assert_eq!(diagnostics.platform, "linux");
        assert!(diagnostics.checks.iter().any(|check| check.name == "dev-net-tun"));
        assert!(diagnostics.checks.iter().any(|check| check.name == "privilege"));
        assert!(diagnostics.checks.iter().any(|check| check.name == "capability-tools"));
        assert!(diagnostics.checks.iter().any(|check| check.name == "iproute2"));
        assert!(!diagnostics.message.contains("http://"));
        assert!(!diagnostics.message.contains("https://"));
        if let Some(manual_action) = diagnostics.manual_action {
            assert!(manual_action.contains("CAP_NET_ADMIN"));
            assert!(!manual_action.contains("http://"));
            assert!(!manual_action.contains("https://"));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_getcap_parser_accepts_standard_capability_output() {
        assert!(super::linux_getcap_has_cap_net_admin(
            "/opt/app/resources/mihomo cap_net_admin=ep\n"
        ));
        assert!(super::linux_getcap_has_cap_net_admin(
            "/opt/app/resources/mihomo cap_net_admin=eip\n"
        ));
        assert!(super::linux_getcap_has_cap_net_admin(
            "/opt/app/resources/mihomo cap_net_admin,cap_net_raw=ep\n"
        ));
        assert!(super::linux_getcap_has_cap_net_admin(
            "/opt/app/resources/mihomo cap_net_raw,cap_net_admin+ep\n"
        ));
        assert!(!super::linux_getcap_has_cap_net_admin(
            "/opt/app/resources/mihomo cap_net_raw=ep\n"
        ));
        assert!(!super::linux_getcap_has_cap_net_admin(
            "/opt/app/resources/mihomo cap_net_admin=p\n"
        ));
        assert!(!super::linux_getcap_has_cap_net_admin(""));
    }

    #[test]
    fn linux_tun_capability_tools_check_explains_non_root_tool_gaps() {
        let missing_tools = super::linux_tun_capability_tools_check(1000, false, false);
        assert_eq!(missing_tools.name, "capability-tools");
        assert!(!missing_tools.ok);
        assert!(missing_tools.message.contains("getcap/setcap"));
        assert!(missing_tools.message.contains("非 root"));

        let getcap_only = super::linux_tun_capability_tools_check(1000, true, false);
        assert!(getcap_only.ok);
        assert!(getcap_only.message.contains("已找到 getcap"));
        assert!(getcap_only.message.contains("未找到 setcap"));

        let root_without_tools = super::linux_tun_capability_tools_check(0, false, false);
        assert!(root_without_tools.ok);
        assert!(root_without_tools.message.contains("当前进程为 root"));
        assert!(root_without_tools.message.contains("libcap"));
    }

    #[test]
    fn linux_tun_manual_action_mentions_tools_and_recovery_without_urls() {
        let tun_device = super::TunCheck {
            name: "dev-net-tun".into(),
            ok: false,
            message: "未找到 /dev/net/tun".into(),
        };
        let privilege = super::TunCheck {
            name: "privilege".into(),
            ok: false,
            message: "当前进程不是 root，且未找到 getcap，无法检测 mihomo CAP_NET_ADMIN".into(),
        };

        let manual_action = super::linux_tun_manual_action(&tun_device, &privilege, false, false);

        assert!(manual_action.contains("/dev/net/tun"));
        assert!(manual_action.contains("setcap cap_net_admin=+ep <mihomo>"));
        assert!(manual_action.contains("getcap/setcap"));
        assert!(manual_action.contains("tun doctor"));
        assert!(manual_action.contains("tun off"));
        assert!(manual_action.contains("core stop"));
        assert!(!manual_action.contains("http://"));
        assert!(!manual_action.contains("https://"));
    }
}
