use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use crossterm::event::KeyCode;
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use serde_json::json;

use super::models::RuleProviderRow;
use super::{
    BusyState, ConfirmAction, ConfirmState, DashboardMetrics, DashboardProxyPopup, DetailState, InputState,
    InputTarget, LogLevelFilter, MIN_TUI_HEIGHT, MIN_TUI_WIDTH, ProviderDialogKind, ProviderSubscriptionInfoRow,
    ProxyGroupLoadSummary, ProxyGroupRow, ProxyNodeMeta, ProxyNodeSort, ProxyPane, ProxyProviderRow, TuiApp,
    TuiInputEvent, View, drain_job_events, normalize_pasted_text, pasted_subscription_url,
    provider_names_for_auto_refresh, proxy_group_load_summary, proxy_groups_empty_message,
    proxy_providers_from_response, render, runtime_proxy_groups_from_yaml, runtime_proxy_summary_from_yaml,
    sanitize_url_error, switch_status_message, terminal_safe_log_text, terminal_safe_text, tui_input_event_trace_line,
    tui_input_events_from_bytes, validate_subscription_url,
};
use crate::jobs::{ClashTuiEvent, ClashTuiEventPayload, JobRecord, JobStatus};
use crate::mihomo_controller::{
    ConnectionMetadata, ConnectionRecord, ControllerHealth, Mode, ProxyGroups, ProxyProvidersResponse, RuleEntry,
};
use crate::subscriptions::{SubscriptionProfileStatus, SubscriptionSweep};
use crate::{actions, options::ClashTuiOptions, state::AppState};
use clash_core::{
    IProfiles, KernelOwner, KernelSnapshot, KernelState, LocalProfileImport, PrfItem,
    config::{PrfExtra, PrfSelected},
};

fn test_job(id: &str, status: JobStatus) -> JobRecord {
    JobRecord {
        id: id.into(),
        kind: "profile-update".into(),
        name: "更新订阅".into(),
        target: Some("remote".into()),
        status,
        message: None,
        error: None,
        result: None,
        created_at: 1,
        updated_at: 2,
        finished_at: None,
    }
}

fn assert_markers_in_order(text: &str, markers: &[&str]) {
    let mut cursor = 0;
    for marker in markers {
        let offset = text[cursor..]
            .find(marker)
            .expect("overview marker should be rendered in order");
        cursor += offset + marker.len();
    }
}

fn test_diagnose_report(recommendations: Vec<String>) -> actions::diagnose::DiagnoseReport {
    actions::diagnose::DiagnoseReport {
        status: actions::diagnose::DiagnoseStatus::NeedsAttention,
        current_profile: Some(actions::diagnose::ProfileBrief {
            uid: Some("remote:test".into()),
            name: Some("测试订阅".into()),
            profile_type: Some("remote".into()),
        }),
        kernel: KernelSnapshot {
            state: KernelState::Running,
            owner: KernelOwner::Detached,
            owner_detail: None,
            pid: Some(42),
            version: Some("test".into()),
            last_error: None,
            last_exit: None,
        },
        controller: actions::diagnose::ControllerProbe {
            health: ControllerHealth {
                healthy: true,
                version: Some("test".into()),
                message: None,
            },
        },
        runtime: actions::diagnose::RuntimeProbe {
            readable: true,
            path: "/tmp/runtime.yaml".into(),
            proxies: 3,
            providers: 0,
            groups: 1,
            rules: 2,
            proxy_types: Vec::new(),
            proxy_samples: Vec::new(),
            provider_samples: Vec::new(),
            group_samples: Vec::new(),
            group_proxy_refs: 1,
            group_provider_refs: 0,
            error: None,
        },
        proxies: actions::diagnose::ProxyProbe {
            ready: false,
            entries: 0,
            groups: 0,
            nodes: 0,
            error: Some("controller empty".into()),
        },
        network: actions::diagnose::NetworkProbe {
            tun: crate::platform::TunDiagnostics {
                platform: "linux".into(),
                enabled: true,
                can_enable: false,
                checks: vec![crate::platform::TunCheck {
                    name: "privilege".into(),
                    ok: false,
                    message: "未检测到 CAP_NET_ADMIN".into(),
                }],
                manual_action: Some("执行 tun off 和 core stop 恢复".into()),
                message: "当前 Linux 环境不满足 TUN 开启条件".into(),
            },
            system_proxy_enabled: true,
            system_proxy: crate::platform::SystemProxyDiagnostics {
                platform: "linux".into(),
                endpoint: crate::platform::SystemProxyEndpoint {
                    host: "127.0.0.1".into(),
                    port: 7897,
                    bypass: "localhost,127.0.0.1".into(),
                },
                auto_apply_supported: true,
                can_auto_apply: false,
                checks: vec![crate::platform::SystemProxyCheck {
                    name: "desktop-session".into(),
                    ok: false,
                    message: "未检测到 DBUS_SESSION_BUS_ADDRESS".into(),
                }],
                manual_action: Some("可手动在桌面系统代理中设置 HTTP/HTTPS/SOCKS 主机 127.0.0.1、端口 7897".into()),
                message: "当前 Linux 环境无法自动应用系统代理".into(),
            },
        },
        logs: actions::diagnose::LogProbe {
            recent: Vec::new(),
            warnings: 0,
            errors: 0,
            last_error: None,
        },
        subscription: None,
        recommendations,
    }
}

#[test]
fn diagnose_status_counts_history_recommendations_and_redacts_urls() {
    let report = test_diagnose_report(vec![
        "订阅更新失败：https://example.invalid/sub?token=secret".into(),
        "TUN 已开启但当前环境不满足基本条件：未检测到 CAP_NET_ADMIN；处理建议：执行 tun off 和 core stop 恢复"
            .into(),
        "系统代理已开启但当前环境无法自动应用：未检测到 DBUS_SESSION_BUS_ADDRESS；处理建议：可手动在桌面系统代理中设置 HTTP/HTTPS/SOCKS 主机 127.0.0.1、端口 7897"
            .into(),
    ]);
    let mut app = TuiApp::default();
    let status = super::diagnose_status_message(&report);

    assert!(status.contains("共3条，按 n 查看"));
    assert!(status.contains("[链接]"));
    assert!(!status.contains("https://example.invalid"));
    app.set_status(status);
    app.record_diagnose_recommendations(&report);

    let history = app.status_history.iter().cloned().collect::<Vec<_>>().join("\n");
    assert!(history.contains("诊断建议 1"));
    assert!(history.contains("诊断建议 2"));
    assert!(history.contains("tun off"));
    assert!(history.contains("core stop"));
    assert!(history.contains("HTTP/HTTPS/SOCKS"));
    assert!(history.contains("127.0.0.1"));
    assert!(!history.contains("https://example.invalid"));
    assert!(!history.contains("token=secret"));
}

#[tokio::test]
async fn renders_p0_views() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("terminal");

    for view in [
        View::Dashboard,
        View::Profiles,
        View::Proxies,
        View::Logs,
        View::Settings,
        View::Rules,
        View::Connections,
        View::Jobs,
    ] {
        let app = TuiApp {
            view,
            status: "test".into(),
            ..TuiApp::default()
        };
        terminal
            .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        assert!(format!("{buffer:?}").contains(view.title()));
        assert!(format!("{buffer:?}").contains("键位："));
    }

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn small_terminal_renders_size_hint() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-small-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let backend = TestBackend::new(60, 16);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let app = TuiApp::default();

    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("终端尺寸不足"));
    assert!(rendered.contains("当前终端：60x16"));
    assert!(rendered.contains(&format!("建议至少：{}x{}", MIN_TUI_WIDTH, MIN_TUI_HEIGHT)));
    assert!(rendered.contains("按 q 退出"));
    assert!(!rendered.contains("键位："));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn transient_modals_render_confirmation_input_and_errors() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-modal-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let backend = TestBackend::new(120, 32);
    let mut terminal = Terminal::new(backend).expect("terminal");

    let app = TuiApp {
        confirm: Some(ConfirmState {
            prompt: "确认清空日志显示和本地日志文件？y 确认 / n 取消".into(),
            action: ConfirmAction::ClearLogs,
        }),
        ..TuiApp::default()
    };
    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw confirm");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("确认操作"));
    assert!(rendered.contains("确认清空日志"));
    assert!(rendered.contains("按 y 确认"));

    let app = TuiApp {
        input: Some(InputState {
            target: InputTarget::ImportSubscriptionUrl,
            value: "https://example.invalid/sub?token=secret".into(),
        }),
        ..TuiApp::default()
    };
    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw input");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("输入"));
    assert!(rendered.contains("订阅链接"));
    assert!(rendered.contains("[订阅链接已输入"));
    assert!(!rendered.contains("https://"));
    assert!(!rendered.contains("token=secret"));

    let app = TuiApp {
        status: "错误：controller unavailable".into(),
        ..TuiApp::default()
    };
    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw error");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("错误提示"));
    assert!(rendered.contains("controller unavailable"));

    let app = TuiApp {
        confirm: Some(ConfirmState {
            prompt: "确认清空日志显示和本地日志文件？y 确认 / n 取消".into(),
            action: ConfirmAction::ClearLogs,
        }),
        busy: Some(BusyState {
            message: "正在清空日志...".into(),
        }),
        ..TuiApp::default()
    };
    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw busy");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("处理中"));
    assert!(rendered.contains("正在清空日志"));
    assert!(!rendered.contains("确认操作"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn busy_messages_cover_long_running_tui_actions_without_leaking_input() {
    let mut app = TuiApp {
        view: View::Dashboard,
        ..TuiApp::default()
    };
    assert_eq!(app.busy_message_for_key(KeyCode::Char('s')), Some("正在启停 Core..."));
    assert_eq!(app.busy_message_for_key(KeyCode::Char('R')), Some("正在重启 Core..."));
    assert_eq!(app.busy_message_for_key(KeyCode::Enter), None);
    app.dashboard_proxy_popup = DashboardProxyPopup::Nodes;
    app.proxy_groups = vec![ProxyGroupRow {
        name: "节点选择".into(),
        now: "香港节点".into(),
        nodes: vec!["香港节点".into()],
        offline: false,
    }];
    assert_eq!(app.busy_message_for_key(KeyCode::Enter), Some("正在切换代理节点..."));

    app.view = View::Profiles;
    assert_eq!(app.busy_message_for_key(KeyCode::Char('s')), Some("正在启停 Core..."));
    assert_eq!(app.busy_message_for_key(KeyCode::Enter), None);
    assert_eq!(
        app.busy_message_for_key(KeyCode::Char('u')),
        Some("正在创建订阅更新任务...")
    );
    app.input = Some(InputState {
        target: InputTarget::ImportSubscriptionUrl,
        value: "https://example.invalid/sub?token=secret".into(),
    });
    assert_eq!(
        app.busy_message_for_key(KeyCode::Enter),
        Some("正在导入订阅并等待代理组加载...")
    );
    assert!(
        app.busy_message_for_key(KeyCode::Enter)
            .is_some_and(|message| !message.contains("example.invalid"))
    );
    app.input = None;

    app.view = View::Settings;
    app.setting_index = 0;
    assert_eq!(app.busy_message_for_key(KeyCode::Enter), Some("正在应用设置..."));
    app.setting_index = super::SETTINGS_ROWS
        .iter()
        .position(|row| *row == super::SettingRow::Tun)
        .expect("tun row");
    assert_eq!(app.busy_message_for_key(KeyCode::Enter), None);

    app.confirm = Some(ConfirmState {
        prompt: "确认清空日志显示和本地日志文件？y 确认 / n 取消".into(),
        action: ConfirmAction::ClearLogs,
    });
    assert_eq!(app.busy_message_for_key(KeyCode::Char('y')), Some("正在清空日志..."));
    assert_eq!(app.busy_message_for_key(KeyCode::Char('n')), None);
    app.confirm = None;

    app.view = View::Jobs;
    assert_eq!(app.busy_message_for_key(KeyCode::Char('c')), Some("正在取消任务..."));
}

#[tokio::test]
async fn dashboard_renders_numbered_navigation_and_actionable_summary() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-dashboard-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let backend = TestBackend::new(150, 34);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let app = TuiApp {
        view: View::Dashboard,
        kernel_snapshot: Some(KernelSnapshot {
            state: KernelState::Running,
            owner: KernelOwner::Systemd,
            owner_detail: Some("clash-tui.service".into()),
            pid: Some(1001),
            version: Some("v1.19.27".into()),
            last_error: Some(
                "failed to download https://example.invalid/sub?token=secret because controller path was too long"
                    .into(),
            ),
            last_exit: None,
        }),
        profiles: vec![PrfItem {
            uid: Some("remote-1".into()),
            itype: Some("remote".into()),
            name: Some("🚀正式订阅".into()),
            extra: Some(PrfExtra {
                upload: 100 * 1024 * 1024,
                download: 200 * 1024 * 1024,
                total: 1000 * 1024 * 1024,
                expire: 0,
            }),
            updated: Some(1),
            ..PrfItem::default()
        }],
        profiles_current: Some("remote-1".into()),
        proxy_groups: vec![ProxyGroupRow {
            name: "🚀节点选择".into(),
            now: "🇭🇰香港节点".into(),
            nodes: vec!["DIRECT".into(), "节点 A".into()],
            offline: false,
        }],
        proxy_node_meta: BTreeMap::from([(
            "🇭🇰香港节点".into(),
            ProxyNodeMeta {
                proxy_type: "ss".into(),
                delay_ms: Some(58),
                alive: Some(true),
            },
        )]),
        dashboard_metrics: DashboardMetrics {
            upload_speed: Some(1024),
            download_speed: Some(2048),
            memory: Some(12 * 1024 * 1024),
        },
        mode: Some(Mode::Rule),
        settings: Some(actions::config::settings(&state).await.expect("settings")),
        ..TuiApp::default()
    };

    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("1 总览"));
    assert!(rendered.contains("2 订阅"));
    assert!(rendered.contains("3 代理"));
    assert!(rendered.contains("运行概览"));
    assert!(rendered.contains("快捷节点"));
    assert!(rendered.contains("快速开关"));
    assert!(rendered.contains("模式切换"));
    assert_markers_in_order(
        &rendered,
        &[
            "核心",
            "客户端",
            "管理方",
            "实时",
            "订阅流量",
            "用量进度",
            "订阅更新",
            "终端类型",
        ],
    );
    assert!(rendered.contains("核心"));
    assert!(rendered.contains("v1.19.27"));
    assert!(rendered.contains("PID 1001"));
    assert!(rendered.contains("管理方"));
    assert!(rendered.contains("systemd"));
    assert!(rendered.contains("clash-tui.service"));
    assert!(rendered.contains("客户端"));
    assert!(rendered.contains(concat!("v", env!("CLASH_TUI_APP_VERSION"))));
    assert!(rendered.contains("实时"));
    assert!(rendered.contains("内存 12.0 MB"));
    assert!(rendered.contains("订阅流量"));
    assert!(rendered.contains("已用 300.0 MB"));
    assert!(rendered.contains("用量进度"));
    assert!(rendered.contains("30.0%"));
    assert!(rendered.contains("订阅更新"));
    assert!(rendered.contains("正式订阅"));
    assert!(!rendered.contains("🚀"));
    assert!(!rendered.contains("🇭🇰"));
    assert!(rendered.contains("代理组"));
    assert!(rendered.contains("节点选择"));
    assert!(rendered.contains("当前节点"));
    assert!(rendered.contains("香港节点"));
    assert!(rendered.contains("58ms"));
    assert_eq!(
        buffer_marker_first_cell(terminal.backend().buffer(), "58ms").fg,
        crate::tui::views::layout::theme_tokens().success
    );
    assert!(rendered.contains("系统代理"));
    assert!(rendered.contains("TUN"));
    assert!(rendered.contains("DNS"));
    assert!(rendered.contains("规则"));
    assert!(!rendered.contains("example.invalid"));
    assert!(!rendered.contains("token=secret"));
    assert!(!rendered.contains("常用入口"));
    assert!(!rendered.contains("网络：mixed-port"));
    assert!(!rendered.contains("接管内核"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn dashboard_cross_lines_join_outer_border() {
    let backend = TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let app = TuiApp::default();

    terminal
        .draw(|frame| crate::tui::views::dashboard::render(Rect::new(0, 0, 120, 30), frame.buffer_mut(), &app))
        .expect("draw");

    let buffer = terminal.backend().buffer();
    assert_eq!(buffer.cell((60, 0)).expect("top divider").symbol(), "┬");
    assert_eq!(buffer.cell((60, 29)).expect("bottom divider").symbol(), "┴");
    assert_eq!(buffer.cell((0, 17)).expect("left divider").symbol(), "├");
    assert_eq!(buffer.cell((119, 17)).expect("right divider").symbol(), "┤");
    assert_eq!(buffer.cell((60, 17)).expect("center divider").symbol(), "┼");
}

#[test]
fn dashboard_proxy_popups_render_scrollable_groups_and_nodes_without_type_status_columns() {
    let backend = TestBackend::new(140, 30);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut app = TuiApp {
        view: View::Dashboard,
        dashboard_proxy_popup: DashboardProxyPopup::Groups,
        dashboard_proxy_group_index: 24,
        proxy_groups: (0..36)
            .map(|index| ProxyGroupRow {
                name: format!("策略组 {index:02}"),
                now: format!("节点 {index:02}"),
                nodes: vec![format!("节点 {index:02}"), "DIRECT".into()],
                offline: false,
            })
            .collect(),
        ..TuiApp::default()
    };

    terminal
        .draw(|frame| crate::tui::views::dashboard::render(frame.area(), frame.buffer_mut(), &app))
        .expect("draw group popup");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("选择代理组"));
    assert!(rendered.contains("策略组 24"));
    assert!(rendered.contains("节点 24"));

    let mut nodes = (0..48).map(|index| format!("节点 {index:02}")).collect::<Vec<_>>();
    nodes[31] = "剩余流量：3.68 TB".into();
    app.dashboard_proxy_popup = DashboardProxyPopup::Nodes;
    app.dashboard_proxy_group_index = 0;
    app.dashboard_proxy_node_index = 31;
    app.proxy_groups = vec![ProxyGroupRow {
        name: "GLOBAL".into(),
        now: "剩余流量：3.68 TB".into(),
        nodes,
        offline: false,
    }];
    app.proxy_node_meta = BTreeMap::from([
        (
            "节点 30".into(),
            ProxyNodeMeta {
                proxy_type: "ss".into(),
                delay_ms: Some(88),
                alive: Some(true),
            },
        ),
        (
            "剩余流量：3.68 TB".into(),
            ProxyNodeMeta {
                proxy_type: "ss".into(),
                delay_ms: Some(0),
                alive: Some(true),
            },
        ),
    ]);
    terminal
        .draw(|frame| crate::tui::views::dashboard::render(frame.area(), frame.buffer_mut(), &app))
        .expect("draw node popup");
    let buffer = terminal.backend().buffer();
    let rendered = format!("{buffer:?}");
    let node_header = buffer_compact_lines(buffer)
        .into_iter()
        .find(|line| line.contains("节点") && line.contains("延迟"))
        .expect("node popup header");
    let node_header_popup = &node_header[node_header
        .find("节点延迟")
        .expect("node popup header should contain compact columns")..];
    assert!(rendered.contains("选择节点"));
    assert!(rendered.contains("节点 27"));
    assert!(rendered.contains("剩余流量"));
    assert!(
        rendered.contains("节点 34") || rendered.contains("节点 35"),
        "node popup should render context after selected row"
    );
    assert!(rendered.contains("延迟"));
    assert!(rendered.contains("88ms"));
    assert!(rendered.contains("Timeout"));
    assert_eq!(
        buffer_marker_first_cell(buffer, "88ms").fg,
        crate::tui::views::layout::theme_tokens().success
    );
    assert!(!rendered.contains("0ms"));
    assert!(rendered.contains("█"));
    assert!(!node_header_popup.contains("类型"));
    assert!(!node_header_popup.contains("状态"));
}

#[tokio::test]
async fn dashboard_keys_open_popups_and_preserve_network_confirmations() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-dashboard-keys-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let settings = actions::config::settings(&state).await.expect("settings");
    let expected_system_proxy_enabled = !settings.system_proxy_enabled;
    let expected_tun_enabled = !settings.tun_enabled;
    let mut app = TuiApp {
        view: View::Dashboard,
        settings: Some(settings),
        proxy_groups: vec![ProxyGroupRow {
            name: "节点选择".into(),
            now: "香港节点".into(),
            nodes: vec!["香港节点".into(), "DIRECT".into()],
            offline: false,
        }],
        ..TuiApp::default()
    };

    assert!(!app.handle_key(KeyCode::Enter, &state).await.expect("enter"));
    assert_eq!(app.dashboard_proxy_popup, DashboardProxyPopup::Nodes);
    assert!(app.confirm.is_none());
    assert!(!app.handle_key(KeyCode::Esc, &state).await.expect("esc"));
    assert_eq!(app.dashboard_proxy_popup, DashboardProxyPopup::None);

    assert!(!app.handle_key(KeyCode::Char('g'), &state).await.expect("group"));
    assert_eq!(app.dashboard_proxy_popup, DashboardProxyPopup::Groups);
    assert!(!app.handle_key(KeyCode::Enter, &state).await.expect("select group"));
    assert_eq!(app.dashboard_proxy_popup, DashboardProxyPopup::Nodes);

    assert!(!app.handle_key(KeyCode::Char('P'), &state).await.expect("system proxy"));
    assert!(matches!(
        app.confirm.as_ref().map(|confirm| &confirm.action),
        Some(ConfirmAction::ToggleSystemProxy { enabled }) if *enabled == expected_system_proxy_enabled
    ));
    app.confirm = None;

    assert!(!app.handle_key(KeyCode::Char('T'), &state).await.expect("tun"));
    assert!(matches!(
        app.confirm.as_ref().map(|confirm| &confirm.action),
        Some(ConfirmAction::ToggleTun { enabled }) if *enabled == expected_tun_enabled
    ));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn dashboard_refresh_populates_summary_sources() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-dashboard-refresh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let mut app = TuiApp {
        view: View::Dashboard,
        ..TuiApp::default()
    };

    app.refresh(&state).await;

    assert!(app.kernel_snapshot.is_some());
    assert!(app.settings.is_some());
    assert!(!app.profiles.is_empty());

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn keyboard_navigation_changes_views_and_quits() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-keys-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let mut app = TuiApp::default();

    assert!(!app.handle_key(KeyCode::Right, &state).await.expect("right"));
    assert_eq!(app.view, View::Profiles);
    assert!(!app.handle_key(KeyCode::Left, &state).await.expect("left"));
    assert_eq!(app.view, View::Dashboard);
    assert!(!app.handle_key(KeyCode::Char('8'), &state).await.expect("8"));
    assert_eq!(app.view, View::Jobs);
    assert!(!app.handle_key(KeyCode::Char('?'), &state).await.expect("help"));
    assert!(app.show_help);
    assert!(app.status.contains("帮助"));
    assert!(!app.handle_key(KeyCode::Esc, &state).await.expect("close help"));
    assert!(!app.show_help);
    assert!(!app.handle_key(KeyCode::Char('?'), &state).await.expect("help again"));
    assert!(app.handle_key(KeyCode::Char('q'), &state).await.expect("quit"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn renders_full_help_screen() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-help-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let backend = TestBackend::new(120, 32);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let app = TuiApp {
        show_help: true,
        ..TuiApp::default()
    };

    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw");
    let buffer = terminal.backend().buffer();
    let rendered = format!("{buffer:?}");
    assert!(rendered.contains("完整键位帮助"));
    assert!(rendered.contains("s 启停核心"));
    assert!(rendered.contains("任意页面直接粘贴"));
    assert!(rendered.contains("D 诊断"));
    assert!(rendered.contains("E 导出诊断快照"));
    assert!(rendered.contains("系统代理会二次确认"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn settings_view_renders_network_apply_boundaries() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-settings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let backend = TestBackend::new(140, 32);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut app = TuiApp {
        view: View::Settings,
        settings: Some(actions::config::settings(&state).await.expect("settings")),
        ..TuiApp::default()
    };

    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("网络接管"));
    assert!(rendered.contains("系统代理"));
    assert!(rendered.contains("TUN"));
    assert!(rendered.contains("系统代理诊断"));
    assert!(rendered.contains("HTTP/HTTPS/SOCKS"));
    assert!(rendered.contains("127.0.0.1:7897"));
    assert!(rendered.contains("TUN 诊断"));
    assert!(rendered.contains("生效边界"));
    assert!(rendered.contains("设置项"));
    assert!(rendered.contains("当前值"));
    assert!(rendered.contains("操作"));
    assert!(rendered.contains("说明"));
    assert!(rendered.contains("终端显示"));
    assert!(rendered.contains("标准"));
    assert!(rendered.contains("中文标点"));
    assert!(rendered.contains("保留"));
    assert!(rendered.contains("核心日志"));
    assert!(rendered.contains("控制 mihomo 日志落盘"));

    let buffer = terminal.backend().buffer();
    let dns_action_column = buffer_marker_column_on_row(buffer, "DNS", "切换");
    let allow_lan_action_column = buffer_marker_column_on_row(buffer, "允许局域网", "切换");
    let terminal_display_action_column = buffer_marker_column_on_row(buffer, "终端显示", "循环");
    let punctuation_action_column = buffer_marker_column_on_row(buffer, "中文标点", "循环");
    let log_level_action_column = buffer_marker_column_on_row(buffer, "日志等级", "循环");
    let core_log_action_column = buffer_marker_column_on_row(buffer, "核心日志", "确认");
    let tun_action_column = buffer_marker_column_on_row(buffer, "TUN", "确认");
    assert_eq!(
        dns_action_column, allow_lan_action_column,
        "settings action column should stay aligned for Chinese rows"
    );
    assert_eq!(
        dns_action_column, log_level_action_column,
        "settings action column should stay aligned for mixed action labels"
    );
    assert_eq!(
        dns_action_column, terminal_display_action_column,
        "terminal display row should use the same action column"
    );
    assert_eq!(
        dns_action_column, punctuation_action_column,
        "punctuation row should use the same action column"
    );
    assert_eq!(
        dns_action_column, core_log_action_column,
        "core log row should use the same action column"
    );
    assert_eq!(
        dns_action_column, tun_action_column,
        "settings action column should stay aligned for ASCII labels"
    );

    app.setting_index = super::settings_rows()
        .iter()
        .position(|row| super::setting_label(*row) == "TUN")
        .expect("tun row");
    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw selected");
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer_marker_column_on_row(buffer, "TUN", ">"), 1);
    assert_eq!(buffer_marker_column_on_row(buffer, "TUN", "确认"), dns_action_column);

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn system_proxy_confirm_prompt_shows_manual_endpoint_when_auto_apply_is_unavailable() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-system-proxy-confirm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let mut app = TuiApp {
        view: View::Settings,
        settings: Some(actions::config::settings(&state).await.expect("settings")),
        setting_index: super::settings_rows()
            .iter()
            .position(|row| *row == super::SettingRow::SystemProxy)
            .expect("system proxy row"),
        ..TuiApp::default()
    };

    assert!(!app.handle_key(KeyCode::Enter, &state).await.expect("enter"));

    let prompt = app.confirm.as_ref().expect("confirm").prompt.as_str();
    assert!(prompt.contains("系统代理"));
    assert!(prompt.contains("HTTP/HTTPS/SOCKS"));
    assert!(prompt.contains("127.0.0.1:7897"));
    assert!(!prompt.contains("http://"));
    assert!(!prompt.contains("https://"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn profile_rows_align_wide_text_columns() {
    assert_eq!(crate::tui::views::layout::display_width("🇺🇸"), 2);
    assert_eq!(crate::tui::views::layout::display_width("🚀"), 2);
    assert_eq!(crate::tui::views::layout::stable_table_text("🚀节点选择"), "节点选择");
    assert_eq!(crate::tui::views::layout::stable_table_text("🇺🇸美国节点"), "美国节点");

    let backend = TestBackend::new(120, 16);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let app = TuiApp {
        view: View::Profiles,
        profiles_current: Some("remote-cn".into()),
        profiles: vec![
            PrfItem {
                uid: Some("remote-cn".into()),
                itype: Some("remote".into()),
                name: Some("🐮正式订阅-香港节点".into()),
                ..PrfItem::default()
            },
            PrfItem {
                uid: Some("local-default".into()),
                itype: Some("local".into()),
                name: Some("default".into()),
                ..PrfItem::default()
            },
        ],
        ..TuiApp::default()
    };

    terminal
        .draw(|frame| crate::tui::views::profiles::render(frame.area(), frame.buffer_mut(), &app))
        .expect("draw profiles table");
    let buffer = terminal.backend().buffer();
    let rendered = format!("{buffer:?}");
    assert!(!rendered.contains("🐮"));
    assert!(rendered.contains("正式订阅-香港节点"));
    assert_eq!(
        buffer_marker_column_on_row(buffer, "正式订阅-香港节点", "远程"),
        buffer_marker_column_on_row(buffer, "default", "本地")
    );

    // Proxies uses ratatui Table columns now; render-level alignment is tested separately.
}

#[test]
fn profile_subscription_status_label_prioritizes_jobs_and_results() {
    let now_secs = 1;
    let base = SubscriptionProfileStatus {
        uid: Some("remote".into()),
        name: Some("正式订阅".into()),
        remote: true,
        auto_update_enabled: true,
        update_interval_minutes: Some(60),
        updated_at: Some(1),
        next_update_at: Some(3601),
        due: false,
        due_reason: "scheduled".into(),
        active_job: None,
        latest_job: None,
        latest_result: None,
        latest_failure: None,
    };

    assert_eq!(
        crate::tui::views::profiles::subscription_status_label(&base, now_secs),
        "已排期：1小时后"
    );

    let mut success = base.clone();
    success.latest_result = Some("profile updated".into());
    assert_eq!(
        crate::tui::views::profiles::subscription_status_label(&success, now_secs),
        "最近成功"
    );

    let mut runtime_success = base.clone();
    runtime_success.latest_result = Some("profile updated; runtime refreshed; core restarted".into());
    assert_eq!(
        crate::tui::views::profiles::subscription_status_label(&runtime_success, now_secs),
        "成功：订阅已更新；运行配置已刷新；核心已重启"
    );

    let mut custom_success = base;
    custom_success.latest_result = Some("saved https://example.invalid/sub?token=secret".into());
    let label = crate::tui::views::profiles::subscription_status_label(&custom_success, now_secs);
    assert!(label.contains("成功：saved [订阅链接]"));
    assert!(!label.contains("https://example.invalid"));

    let mut due = success;
    due.due = true;
    assert_eq!(
        crate::tui::views::profiles::subscription_status_label(&due, now_secs),
        "到期"
    );

    let mut failed_due = due;
    failed_due.latest_failure = Some("download failed https://example.invalid/sub?token=secret".into());
    let label = crate::tui::views::profiles::subscription_status_label(&failed_due, now_secs);
    assert!(label.contains("失败：download failed [订阅链接]"));
    assert!(!label.contains("https://example.invalid"));

    let mut running_failed_due = failed_due;
    running_failed_due.active_job = Some(test_job("job-running", JobStatus::Running));
    assert_eq!(
        crate::tui::views::profiles::subscription_status_label(&running_failed_due, now_secs),
        "任务运行中"
    );
}

#[test]
fn subscription_time_labels_are_readable_and_skip_disabled_profiles() {
    assert_eq!(super::seconds_until_label(0), "现在");
    assert_eq!(super::seconds_until_label(30), "30秒后");
    assert_eq!(super::seconds_until_label(61), "2分钟后");
    assert_eq!(super::seconds_until_label(3661), "1小时2分钟后");
    assert_eq!(super::seconds_until_label(7199), "2小时后");
    assert_eq!(super::seconds_until_label(86_399), "1天后");
    assert_eq!(super::seconds_until_label(86_401), "2天后");

    let active = SubscriptionProfileStatus {
        uid: Some("remote".into()),
        name: Some("自动更新".into()),
        remote: true,
        auto_update_enabled: true,
        update_interval_minutes: Some(60),
        updated_at: Some(1),
        next_update_at: Some(121),
        due: false,
        due_reason: "scheduled".into(),
        active_job: None,
        latest_job: None,
        latest_result: None,
        latest_failure: None,
    };
    let mut disabled = active.clone();
    disabled.auto_update_enabled = false;
    disabled.next_update_at = Some(61);

    assert_eq!(
        crate::tui::views::profiles::next_profile_update_label(&[disabled, active], 1),
        "2分钟后"
    );
}

#[test]
fn subscription_sweep_status_message_lists_jobs_and_empty_state() {
    let sweep = SubscriptionSweep {
        checked: 5,
        due: 4,
        queued: 2,
        skipped: 2,
        jobs: vec![
            test_job("job-1", JobStatus::Pending),
            test_job("job-2", JobStatus::Running),
            test_job("job-3", JobStatus::Running),
            test_job("job-4", JobStatus::Running),
        ],
    };

    let message = super::subscription_sweep_status_message(&sweep);
    assert!(message.contains("检查=5"));
    assert!(message.contains("远程=4"));
    assert!(message.contains("入队=2"));
    assert!(message.contains("已运行=2"));
    assert!(message.contains("任务=job-1, job-2, job-3 等 1 个"));
    assert!(message.contains("按 8 查看详情"));

    let empty = SubscriptionSweep {
        checked: 1,
        due: 0,
        queued: 0,
        skipped: 0,
        jobs: Vec::new(),
    };
    assert!(super::subscription_sweep_status_message(&empty).contains("没有可更新的远程订阅"));
}

#[tokio::test]
async fn proxy_table_preserves_selection_and_aligns_mixed_width_columns() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-proxy-table-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let groups = vec![
        ProxyGroupRow {
            name: "🚀节点选择".into(),
            now: "未预选".into(),
            nodes: vec!["HK".into(), "JP".into()],
            offline: true,
        },
        ProxyGroupRow {
            name: "🇯🇵日本节点".into(),
            now: "未预选".into(),
            nodes: vec!["JP".into()],
            offline: true,
        },
        ProxyGroupRow {
            name: "🇺🇸美国节点".into(),
            now: "未预选".into(),
            nodes: vec!["US".into()],
            offline: true,
        },
    ];
    let mut app = TuiApp {
        view: View::Proxies,
        proxy_groups: groups.clone(),
        ..TuiApp::default()
    };

    app.move_selection(1);
    app.move_selection(1);
    assert_eq!(
        app.selected_proxy_group().map(|group| group.name.as_str()),
        Some("🇺🇸美国节点")
    );

    app.apply_proxy_groups(Vec::new());
    app.apply_proxy_groups(groups);
    assert_eq!(
        app.selected_proxy_group().map(|group| group.name.as_str()),
        Some("🇺🇸美国节点")
    );

    let backend = TestBackend::new(140, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw proxies table");
    let buffer = terminal.backend().buffer();
    let rendered = format!("{buffer:?}");
    assert!(!rendered.contains("🚀"));
    assert!(!rendered.contains("🇯🇵"));
    assert!(!rendered.contains("🇺🇸"));
    assert!(rendered.contains("节点选择"));
    assert!(rendered.contains("日本节点"));
    assert!(rendered.contains("美国节点"));
    assert!(rendered.contains("状态：runtime预选，可预选"));
    assert!(!rendered.contains("来源"));
    let now_columns = buffer_marker_columns(buffer, "未预选");
    assert_eq!(now_columns.len(), 3);
    assert!(now_columns.windows(2).all(|pair| pair[0] == pair[1]));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn proxy_group_selection_restores_from_current_profile_selected() {
    let groups = vec![
        ProxyGroupRow {
            name: "GLOBAL".into(),
            now: "HK".into(),
            nodes: vec!["HK".into()],
            offline: false,
        },
        ProxyGroupRow {
            name: "日本节点".into(),
            now: "JP".into(),
            nodes: vec!["JP".into()],
            offline: false,
        },
    ];
    let mut app = TuiApp::default();

    app.apply_profiles(IProfiles {
        current: Some("R001".into()),
        items: Some(vec![PrfItem {
            uid: Some("R001".into()),
            selected: Some(vec![
                PrfSelected {
                    name: Some("GLOBAL".into()),
                    now: Some("HK".into()),
                },
                PrfSelected {
                    name: Some("日本节点".into()),
                    now: Some("JP".into()),
                },
            ]),
            ..PrfItem::default()
        }]),
    });
    app.apply_proxy_groups(groups);

    assert_eq!(
        app.selected_proxy_group().map(|group| group.name.as_str()),
        Some("日本节点")
    );
    assert_eq!(app.proxy_group_selection_key.as_deref(), Some("日本节点"));
}

#[test]
fn proxy_group_selection_restores_from_profile_when_groups_load_first() {
    let groups = vec![
        ProxyGroupRow {
            name: "GLOBAL".into(),
            now: "HK".into(),
            nodes: vec!["HK".into()],
            offline: false,
        },
        ProxyGroupRow {
            name: "日本节点".into(),
            now: "JP".into(),
            nodes: vec!["JP".into()],
            offline: false,
        },
    ];
    let mut app = TuiApp::default();

    app.apply_proxy_groups(groups.clone());
    assert_eq!(
        app.selected_proxy_group().map(|group| group.name.as_str()),
        Some("GLOBAL")
    );

    app.apply_profiles(IProfiles {
        current: Some("R001".into()),
        items: Some(vec![PrfItem {
            uid: Some("R001".into()),
            selected: Some(vec![PrfSelected {
                name: Some("日本节点".into()),
                now: Some("JP".into()),
            }]),
            ..PrfItem::default()
        }]),
    });

    assert_eq!(app.proxy_group_selection_key.as_deref(), Some("日本节点"));
    assert!(app.selected_proxy_group().is_none());

    app.apply_proxy_groups(groups);

    assert_eq!(
        app.selected_proxy_group().map(|group| group.name.as_str()),
        Some("日本节点")
    );
    assert_eq!(app.proxy_group_selection_key.as_deref(), Some("日本节点"));
}

#[tokio::test]
async fn dashboard_proxy_group_confirm_only_focuses_nodes_without_persisting_profile() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-dashboard-group-save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    state
        .store
        .import_local_profile(&LocalProfileImport {
            uid: Some("L001".into()),
            name: Some("Demo".into()),
            file_data: "proxies: []\nproxy-groups: []\nrules: []\n".into(),
        })
        .await
        .expect("profile");
    let mut app = TuiApp {
        view: View::Dashboard,
        dashboard_proxy_popup: DashboardProxyPopup::Groups,
        dashboard_proxy_group_index: 1,
        proxy_groups: vec![
            ProxyGroupRow {
                name: "GLOBAL".into(),
                now: "HK".into(),
                nodes: vec!["HK".into()],
                offline: false,
            },
            ProxyGroupRow {
                name: "日本节点".into(),
                now: "JP".into(),
                nodes: vec!["JP".into()],
                offline: false,
            },
        ],
        ..TuiApp::default()
    };

    app.activate_dashboard(&state).await;

    let profiles = state.store.load_profiles().await.expect("profiles");
    assert!(
        profiles
            .get_item("L001")
            .expect("profile item")
            .selected
            .as_ref()
            .is_none_or(Vec::is_empty),
        "selecting a group must not persist proxy selection"
    );
    assert_eq!(app.dashboard_proxy_group_selection_key.as_deref(), Some("日本节点"));
    assert_eq!(app.dashboard_proxy_node_selection_key.as_deref(), Some("JP"));
    assert_eq!(app.proxy_group_selection_key.as_deref(), None);
    assert_eq!(app.dashboard_proxy_popup, DashboardProxyPopup::Nodes);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn proxy_group_navigation_does_not_change_dashboard_group_context() {
    let mut app = TuiApp {
        view: View::Dashboard,
        proxy_groups: vec![
            ProxyGroupRow {
                name: "GLOBAL".into(),
                now: "香港节点".into(),
                nodes: vec!["香港节点".into()],
                offline: false,
            },
            ProxyGroupRow {
                name: "日本节点".into(),
                now: "JP".into(),
                nodes: vec!["JP".into()],
                offline: false,
            },
            ProxyGroupRow {
                name: "美国节点".into(),
                now: "US".into(),
                nodes: vec!["US".into()],
                offline: false,
            },
        ],
        dashboard_proxy_group_index: 1,
        dashboard_proxy_group_selection_key: Some("日本节点".into()),
        ..TuiApp::default()
    };
    app.focus_dashboard_proxy_nodes_on_current();

    app.set_view(View::Proxies);
    app.proxy_pane = ProxyPane::Groups;
    app.move_selection(2);

    assert_eq!(
        app.selected_proxy_group().map(|group| group.name.as_str()),
        Some("美国节点")
    );
    assert_eq!(
        app.dashboard_proxy_group().map(|group| group.name.as_str()),
        Some("日本节点")
    );
    assert_eq!(app.dashboard_proxy_group_selection_key.as_deref(), Some("日本节点"));
}

#[test]
fn dashboard_proxy_group_popup_selection_survives_profile_refresh() {
    let profiles = IProfiles {
        current: Some("R001".into()),
        items: Some(vec![PrfItem {
            uid: Some("R001".into()),
            selected: Some(vec![PrfSelected {
                name: Some("订阅策略".into()),
                now: Some("香港三区".into()),
            }]),
            ..PrfItem::default()
        }]),
    };
    let groups = vec![
        ProxyGroupRow {
            name: "GLOBAL".into(),
            now: "美国四区".into(),
            nodes: vec!["美国四区".into()],
            offline: false,
        },
        ProxyGroupRow {
            name: "订阅策略".into(),
            now: "香港三区".into(),
            nodes: vec!["香港三区".into()],
            offline: false,
        },
        ProxyGroupRow {
            name: "故障转移".into(),
            now: "美国五区".into(),
            nodes: vec!["美国五区".into()],
            offline: false,
        },
        ProxyGroupRow {
            name: "自动选择".into(),
            now: "香港三区".into(),
            nodes: vec!["香港三区".into()],
            offline: false,
        },
    ];
    let mut app = TuiApp {
        view: View::Dashboard,
        ..TuiApp::default()
    };
    app.apply_profiles(profiles.clone());
    app.apply_proxy_groups(groups.clone());
    app.open_dashboard_proxy_groups();

    assert_eq!(
        app.dashboard_proxy_group().map(|group| group.name.as_str()),
        Some("订阅策略")
    );

    app.move_selection(1);

    assert!(app.dashboard_proxy_user_selection_is_sticky());
    assert_eq!(
        app.dashboard_proxy_group().map(|group| group.name.as_str()),
        Some("故障转移")
    );
    assert_eq!(app.dashboard_proxy_group_selection_key.as_deref(), Some("故障转移"));

    app.apply_profiles(profiles);
    app.apply_proxy_groups(groups);

    assert_eq!(app.dashboard_proxy_popup, DashboardProxyPopup::Groups);
    assert_eq!(
        app.dashboard_proxy_group().map(|group| group.name.as_str()),
        Some("故障转移")
    );
    assert_eq!(app.dashboard_proxy_group_selection_key.as_deref(), Some("故障转移"));
    assert_eq!(app.proxy_group_selection_key.as_deref(), Some("订阅策略"));
}

#[test]
fn dashboard_node_selection_updates_named_group_without_moving_proxy_page_cursor() {
    let mut app = TuiApp {
        view: View::Dashboard,
        proxy_pane: ProxyPane::Groups,
        proxy_group_index: 0,
        proxy_groups: vec![
            ProxyGroupRow {
                name: "GLOBAL".into(),
                now: "香港节点".into(),
                nodes: vec!["香港节点".into(), "日本节点".into()],
                offline: false,
            },
            ProxyGroupRow {
                name: "美国节点".into(),
                now: "US-1".into(),
                nodes: vec!["US-1".into(), "US-2".into()],
                offline: false,
            },
        ],
        dashboard_proxy_group_index: 1,
        dashboard_proxy_node_index: 1,
        ..TuiApp::default()
    };

    app.set_selected_proxy_group_now("美国节点", "US-2");

    assert_eq!(app.proxy_group_index, 0);
    assert_eq!(app.proxy_groups[1].now, "US-2");
    assert_eq!(app.dashboard_proxy_node_index, 1);
    assert_eq!(app.proxy_group_selection_key.as_deref(), None);
    assert_eq!(app.dashboard_proxy_group_selection_key.as_deref(), Some("美国节点"));
    assert_eq!(app.dashboard_proxy_node_selection_key.as_deref(), Some("US-2"));
}

#[test]
fn proxy_refresh_keeps_current_selection_when_cached_key_is_stale() {
    let groups = vec![
        ProxyGroupRow {
            name: "🚀节点选择".into(),
            now: "离线预览".into(),
            nodes: vec!["HK".into()],
            offline: true,
        },
        ProxyGroupRow {
            name: "♻自动选择".into(),
            now: "离线预览".into(),
            nodes: vec!["Auto".into()],
            offline: true,
        },
        ProxyGroupRow {
            name: "🇺🇸美国节点".into(),
            now: "离线预览".into(),
            nodes: vec!["US".into()],
            offline: true,
        },
    ];
    let mut app = TuiApp {
        view: View::Proxies,
        proxy_group_index: 2,
        proxy_group_selection_key: Some("🚀节点选择".into()),
        proxy_groups: groups.clone(),
        ..TuiApp::default()
    };

    app.apply_proxy_groups(groups);

    assert_eq!(
        app.selected_proxy_group().map(|group| group.name.as_str()),
        Some("🇺🇸美国节点")
    );
    assert_eq!(app.proxy_group_selection_key.as_deref(), Some("🇺🇸美国节点"));
}

#[test]
fn proxy_refresh_prefers_recent_user_selection_over_stale_index() {
    let groups = vec![
        ProxyGroupRow {
            name: "🚀节点选择".into(),
            now: "离线预览".into(),
            nodes: vec!["HK".into()],
            offline: true,
        },
        ProxyGroupRow {
            name: "♻自动选择".into(),
            now: "离线预览".into(),
            nodes: vec!["Auto".into()],
            offline: true,
        },
        ProxyGroupRow {
            name: "🇯🇵日本节点".into(),
            now: "离线预览".into(),
            nodes: vec!["JP".into()],
            offline: true,
        },
        ProxyGroupRow {
            name: "🇺🇸美国节点".into(),
            now: "离线预览".into(),
            nodes: vec!["US".into()],
            offline: true,
        },
    ];
    let mut app = TuiApp {
        view: View::Proxies,
        proxy_group_index: 0,
        proxy_group_selection_key: Some("🇺🇸美国节点".into()),
        proxy_user_selection_at: Some(Instant::now()),
        proxy_groups: groups.clone(),
        ..TuiApp::default()
    };

    app.apply_proxy_groups(groups);

    assert_eq!(
        app.selected_proxy_group().map(|group| group.name.as_str()),
        Some("🇺🇸美国节点")
    );
    assert_eq!(app.proxy_group_selection_key.as_deref(), Some("🇺🇸美国节点"));
}

#[test]
fn proxy_group_selection_survives_repeated_refresh_after_scrolling() {
    let groups = vec![
        ProxyGroupRow {
            name: "🚀节点选择".into(),
            now: "离线预览".into(),
            nodes: vec!["HK".into()],
            offline: true,
        },
        ProxyGroupRow {
            name: "♻自动选择".into(),
            now: "离线预览".into(),
            nodes: vec!["Auto".into()],
            offline: true,
        },
        ProxyGroupRow {
            name: "🇯🇵日本节点".into(),
            now: "离线预览".into(),
            nodes: vec!["JP".into()],
            offline: true,
        },
        ProxyGroupRow {
            name: "🇺🇸美国节点".into(),
            now: "离线预览".into(),
            nodes: vec!["US".into()],
            offline: true,
        },
    ];
    let mut app = TuiApp {
        view: View::Proxies,
        proxy_groups: groups.clone(),
        ..TuiApp::default()
    };

    for _ in 0..3 {
        app.move_selection(1);
    }
    assert_eq!(
        app.selected_proxy_group().map(|group| group.name.as_str()),
        Some("🇺🇸美国节点")
    );
    assert_eq!(app.proxy_group_selection_key.as_deref(), Some("🇺🇸美国节点"));

    for _ in 0..5 {
        app.apply_proxy_groups(groups.clone());
        assert_eq!(
            app.selected_proxy_group().map(|group| group.name.as_str()),
            Some("🇺🇸美国节点")
        );
        assert_eq!(app.proxy_group_selection_key.as_deref(), Some("🇺🇸美国节点"));
    }
}

#[test]
fn proxy_selection_restore_matches_stable_display_name_after_icon_changes() {
    let mut app = TuiApp {
        view: View::Proxies,
        proxy_group_selection_key: Some("🇺🇸美国节点".into()),
        proxy_node_selection_key: Some("🇭🇰香港 01".into()),
        proxy_pane: ProxyPane::Nodes,
        ..TuiApp::default()
    };

    app.apply_proxy_groups(vec![
        ProxyGroupRow {
            name: "日本节点".into(),
            now: "离线预览".into(),
            nodes: vec!["日本 01".into()],
            offline: true,
        },
        ProxyGroupRow {
            name: "美国节点".into(),
            now: "香港 01".into(),
            nodes: vec!["日本 01".into(), "香港 01".into()],
            offline: true,
        },
    ]);

    assert_eq!(app.proxy_group_index, 1);
    assert_eq!(app.proxy_node_index, 1);
    assert_eq!(app.proxy_group_selection_key.as_deref(), Some("美国节点"));
    assert_eq!(app.proxy_node_selection_key.as_deref(), Some("香港 01"));
}

#[test]
fn rules_connections_jobs_rows_align_wide_text_columns() {
    let mut terminal = Terminal::new(TestBackend::new(140, 18)).expect("rules terminal");
    let app = TuiApp {
        view: View::Rules,
        rules: vec![
            RuleEntry {
                r#type: Some("DOMAIN-SUFFIX".into()),
                payload: Some("视频网站.example".into()),
                proxy: Some("🚀香港自动选择".into()),
                ..RuleEntry::default()
            },
            RuleEntry {
                r#type: Some("MATCH".into()),
                payload: Some("-".into()),
                proxy: Some("DIRECT".into()),
                ..RuleEntry::default()
            },
        ],
        ..TuiApp::default()
    };
    terminal
        .draw(|frame| crate::tui::views::rules::render(frame.area(), frame.buffer_mut(), &app))
        .expect("draw rules table");
    let buffer = terminal.backend().buffer();
    let rendered = format!("{buffer:?}");
    assert!(!rendered.contains("🚀"));
    assert!(rendered.contains("香港自动选择"));
    assert_eq!(
        buffer_marker_column_on_row(buffer, "视频网站.example", "香港自动选择"),
        buffer_marker_column_on_row(buffer, "MATCH", "DIRECT")
    );

    let mut cn_connection = sample_connection();
    cn_connection.id = "conn-中文-1".into();
    cn_connection.metadata.as_mut().expect("metadata").host = Some("视频.example.com".into());
    cn_connection.metadata.as_mut().expect("metadata").process = Some("浏览器进程".into());
    cn_connection.chains = vec!["香港节点".into(), "自动选择".into()];
    let mut ascii_connection = sample_connection();
    ascii_connection.id = "conn-2".into();
    ascii_connection.metadata.as_mut().expect("metadata").host = Some("api.example.com".into());
    ascii_connection.metadata.as_mut().expect("metadata").process = Some("curl".into());
    ascii_connection.rule = Some("MATCH".into());
    ascii_connection.chains = vec!["DIRECT".into()];
    let mut terminal = Terminal::new(TestBackend::new(140, 18)).expect("connections terminal");
    let app = TuiApp {
        view: View::Connections,
        connections: vec![cn_connection, ascii_connection],
        ..TuiApp::default()
    };
    terminal
        .draw(|frame| crate::tui::views::connections::render(frame.area(), frame.buffer_mut(), &app))
        .expect("draw connections table");
    let buffer = terminal.backend().buffer();
    assert_eq!(
        buffer_marker_column_on_row(buffer, "视频.example.com", "香港节点"),
        buffer_marker_column_on_row(buffer, "api.example.com", "DIRECT")
    );

    let mut terminal = Terminal::new(TestBackend::new(150, 18)).expect("jobs terminal");
    let app = TuiApp {
        view: View::Jobs,
        jobs: vec![
            JobRecord {
                id: "job-中文订阅".into(),
                kind: "profile-update".into(),
                name: "更新订阅".into(),
                target: Some("香港订阅".into()),
                status: JobStatus::Failed,
                message: Some("下载失败".into()),
                error: None,
                result: None,
                created_at: 1,
                updated_at: 2,
                finished_at: None,
            },
            JobRecord {
                id: "job-2".into(),
                kind: "profile-update".into(),
                name: "更新订阅".into(),
                target: Some("default".into()),
                status: JobStatus::Succeeded,
                message: Some("profile updated".into()),
                error: None,
                result: None,
                created_at: 1,
                updated_at: 2,
                finished_at: Some(3),
            },
        ],
        ..TuiApp::default()
    };
    terminal
        .draw(|frame| crate::tui::views::jobs::render(frame.area(), frame.buffer_mut(), &app))
        .expect("draw jobs table");
    let buffer = terminal.backend().buffer();
    assert_eq!(
        buffer_marker_column_on_row(buffer, "job-中文订阅", "下载失败"),
        buffer_marker_column_on_row(buffer, "job-2", "订阅已更新")
    );
}

#[test]
fn connection_filter_matches_process_and_ip_metadata() {
    let mut app = TuiApp {
        view: View::Connections,
        connections: vec![sample_connection()],
        connection_query: "93.184".into(),
        ..TuiApp::default()
    };

    assert_eq!(app.filtered_connection_indices(), vec![0]);

    app.connection_query = "curl".into();
    assert_eq!(app.filtered_connection_indices(), vec![0]);

    app.connection_query = "not-found".into();
    assert!(app.filtered_connection_indices().is_empty());
}

#[test]
fn connections_detail_modal_sanitizes_extra_urls() {
    let backend = TestBackend::new(120, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut app = TuiApp {
        view: View::Connections,
        connections: vec![sample_connection()],
        ..TuiApp::default()
    };

    app.open_selected_connection_detail();
    terminal
        .draw(|frame| super::render_transient_modal(frame.area(), frame.buffer_mut(), &app))
        .expect("draw detail");
    let rendered = format!("{:?}", terminal.backend().buffer());

    assert!(rendered.contains("连接详情"));
    assert!(rendered.contains("连接ID"));
    assert!(rendered.contains("目标地址"));
    assert!(rendered.contains("93.184.216.34"));
    assert!(rendered.contains("curl"));
    assert!(rendered.contains("[订阅链接]"));
    assert!(!rendered.contains("https://example.invalid"));
}

#[tokio::test]
async fn connections_enter_opens_detail_and_escape_closes() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-connection-detail-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let mut app = TuiApp {
        view: View::Connections,
        connections: vec![sample_connection()],
        ..TuiApp::default()
    };

    assert!(!app.handle_key(KeyCode::Enter, &state).await.expect("open detail"));
    assert!(app.detail.is_some());
    assert!(app.status.contains("正在查看连接"));
    assert!(!app.handle_key(KeyCode::Esc, &state).await.expect("close detail"));
    assert!(app.detail.is_none());
    assert_eq!(app.status, "已关闭详情");

    let _ = std::fs::remove_dir_all(root);
}

fn sample_connection() -> ConnectionRecord {
    ConnectionRecord {
        id: "conn-secret-url".into(),
        upload: 128,
        download: 4096,
        start: Some("2026-06-18T14:30:00+08:00".into()),
        chains: vec!["香港节点".into(), "自动选择".into()],
        rule: Some("DOMAIN-SUFFIX".into()),
        rule_payload: Some("example.com".into()),
        metadata: Some(ConnectionMetadata {
            network: Some("tcp".into()),
            r#type: Some("HTTP".into()),
            source_ip: Some("198.18.0.1".into()),
            destination_ip: Some("93.184.216.34".into()),
            source_port: Some("51234".into()),
            destination_port: Some("443".into()),
            host: Some("video.example.com".into()),
            process: Some("curl".into()),
            extra: Default::default(),
        }),
        extra: serde_json::from_value(json!({
            "downloadUrl": "https://example.invalid/sub?token=secret"
        }))
        .expect("extra map"),
    }
}

#[test]
fn jobs_view_sanitizes_url_details() {
    let backend = TestBackend::new(160, 12);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let app = TuiApp {
        view: View::Jobs,
        jobs: vec![JobRecord {
            id: "job-secret-url".into(),
            kind: "profile-update".into(),
            name: "更新订阅".into(),
            target: Some("remote".into()),
            status: JobStatus::Failed,
            message: None,
            error: Some("download failed https://example.invalid/sub?token=secret".into()),
            result: None,
            created_at: 1,
            updated_at: 2,
            finished_at: Some(2),
        }],
        ..TuiApp::default()
    };

    terminal
        .draw(|frame| crate::tui::views::jobs::render(frame.area(), frame.buffer_mut(), &app))
        .expect("draw");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("[订阅链接]"));
    assert!(!rendered.contains("https://example.invalid"));
    assert!(rendered.contains("任务ID"));
    assert!(rendered.contains("状态"));
    assert!(rendered.contains("详情"));
}

#[test]
fn jobs_view_summarizes_batch_status_and_filters_by_result_text() {
    let backend = TestBackend::new(160, 18);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut app = TuiApp {
        view: View::Jobs,
        jobs: vec![
            JobRecord {
                id: "job-pending".into(),
                kind: "profile-update".into(),
                name: "更新订阅 A".into(),
                target: Some("remote-a".into()),
                status: JobStatus::Pending,
                message: None,
                error: None,
                result: None,
                created_at: 1,
                updated_at: 1,
                finished_at: None,
            },
            JobRecord {
                id: "job-running".into(),
                kind: "profile-update".into(),
                name: "更新订阅 B".into(),
                target: Some("remote-b".into()),
                status: JobStatus::Running,
                message: Some("downloading".into()),
                error: None,
                result: None,
                created_at: 2,
                updated_at: 3,
                finished_at: None,
            },
            JobRecord {
                id: "job-ok".into(),
                kind: "profile-update".into(),
                name: "更新订阅 C".into(),
                target: Some("remote-c".into()),
                status: JobStatus::Succeeded,
                message: Some("profile updated".into()),
                error: None,
                result: None,
                created_at: 3,
                updated_at: 4,
                finished_at: Some(4),
            },
            JobRecord {
                id: "job-failed".into(),
                kind: "profile-update".into(),
                name: "更新订阅 D".into(),
                target: Some("remote-d".into()),
                status: JobStatus::Failed,
                message: None,
                error: Some("下载失败 https://example.invalid/sub?token=secret".into()),
                result: None,
                created_at: 4,
                updated_at: 5,
                finished_at: Some(5),
            },
            JobRecord {
                id: "job-cancelled".into(),
                kind: "diagnose".into(),
                name: "诊断".into(),
                target: None,
                status: JobStatus::Cancelled,
                message: None,
                error: None,
                result: None,
                created_at: 5,
                updated_at: 6,
                finished_at: Some(6),
            },
        ],
        ..TuiApp::default()
    };

    terminal
        .draw(|frame| crate::tui::views::jobs::render(frame.area(), frame.buffer_mut(), &app))
        .expect("draw jobs");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("等待：1"));
    assert!(rendered.contains("运行：1"));
    assert!(rendered.contains("成功：1"));
    assert!(rendered.contains("失败：1"));
    assert!(rendered.contains("取消：1"));
    assert!(rendered.contains("可重试：1"));
    assert!(rendered.contains("关注：运行中"));
    assert!(rendered.contains("最近失败"));
    assert!(rendered.contains("摘要"));
    assert!(rendered.contains("[订阅链接]"));
    assert!(!rendered.contains("https://example.invalid"));

    app.job_query = "失败".into();
    assert_eq!(app.filtered_job_indices(), vec![3]);
    app.job_query = "downloading".into();
    assert_eq!(app.filtered_job_indices(), vec![1]);
    app.job_query = "已取消".into();
    assert_eq!(app.filtered_job_indices(), vec![4]);
}

#[test]
fn jobs_detail_modal_sanitizes_message_and_result_urls() {
    let backend = TestBackend::new(120, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut app = TuiApp {
        view: View::Jobs,
        jobs: vec![JobRecord {
            id: "job-detail".into(),
            kind: "profile-update".into(),
            name: "更新 https://example.invalid/name?token=secret".into(),
            target: Some("remote".into()),
            status: JobStatus::Failed,
            message: Some("download failed https://example.invalid/sub?token=secret".into()),
            error: None,
            result: Some(json!({
                "url": "https://example.invalid/result?token=secret",
                "message": "failed"
            })),
            created_at: 1,
            updated_at: 2,
            finished_at: Some(2),
        }],
        ..TuiApp::default()
    };

    app.open_selected_job_detail();
    terminal
        .draw(|frame| super::render_transient_modal(frame.area(), frame.buffer_mut(), &app))
        .expect("draw detail");
    let rendered = format!("{:?}", terminal.backend().buffer());

    assert!(rendered.contains("任务详情"));
    assert!(rendered.contains("任务ID"));
    assert!(rendered.contains("[订阅链接]"));
    assert!(rendered.contains("按 Esc/Enter 关闭详情"));
    assert!(!rendered.contains("https://example.invalid"));
}

#[tokio::test]
async fn jobs_detail_modal_closes_with_escape_without_exiting() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-job-detail-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let mut app = TuiApp {
        view: View::Jobs,
        jobs: vec![JobRecord {
            id: "job-detail-close".into(),
            kind: "profile-update".into(),
            name: "更新订阅".into(),
            target: Some("remote".into()),
            status: JobStatus::Succeeded,
            message: Some("profile updated".into()),
            error: None,
            result: None,
            created_at: 1,
            updated_at: 2,
            finished_at: Some(2),
        }],
        ..TuiApp::default()
    };

    assert!(!app.handle_key(KeyCode::Enter, &state).await.expect("open detail"));
    assert!(app.detail.is_some());
    assert!(!app.handle_key(KeyCode::Esc, &state).await.expect("close detail"));
    assert!(app.detail.is_none());
    assert_eq!(app.status, "已关闭详情");

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn proxy_page_keeps_provider_data_out_of_main_focus() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-provider-hidden-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let backend = TestBackend::new(150, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let app = TuiApp {
        view: View::Proxies,
        proxy_pane: ProxyPane::Nodes,
        proxy_groups: vec![ProxyGroupRow {
            name: "GLOBAL".into(),
            now: "香港节点".into(),
            nodes: vec!["香港节点".into(), "日本节点".into()],
            offline: false,
        }],
        ..TuiApp::default()
    };

    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw proxy page");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("代理组"));
    assert!(rendered.contains("节点"));
    assert!(rendered.contains("香港节点"));
    assert!(!rendered.contains("最近操作"));
    assert!(!rendered.contains("远程Provider"));
    assert!(!rendered.contains("备用Provider"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn provider_dialog_renders_proxy_provider_rows_without_third_focus() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-provider-dialog-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let backend = TestBackend::new(150, 28);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut app = TuiApp {
        view: View::Proxies,
        proxy_pane: ProxyPane::Nodes,
        proxy_groups: vec![ProxyGroupRow {
            name: "GLOBAL".into(),
            now: "香港节点".into(),
            nodes: vec!["香港节点".into(), "日本节点".into()],
            offline: false,
        }],
        proxy_providers: vec![
            ProxyProviderRow {
                name: "远程 Provider/香港".into(),
                provider_type: "Proxy".into(),
                vehicle_type: "HTTP".into(),
                proxy_count: 164,
                updated_at: Some("2026-06-18T18:00:00+08:00".into()),
                subscription: Some(ProviderSubscriptionInfoRow {
                    upload: Some(1024 * 1024),
                    download: Some(2 * 1024 * 1024),
                    total: Some(10 * 1024 * 1024),
                    expire: Some(4_102_444_800),
                }),
            },
            ProxyProviderRow {
                name: "备用 Provider".into(),
                provider_type: "Proxy".into(),
                vehicle_type: "File".into(),
                proxy_count: 0,
                updated_at: None,
                subscription: None,
            },
            ProxyProviderRow {
                name: "零时间 Provider".into(),
                provider_type: "Compatible".into(),
                vehicle_type: "Compatible".into(),
                proxy_count: 99,
                updated_at: Some("0001-01-01T00:00:00Z".into()),
                subscription: None,
            },
        ],
        provider_dialog: Some(ProviderDialogKind::Proxy),
        ..TuiApp::default()
    };
    app.set_provider_feedback(ProviderDialogKind::Proxy, "远程 Provider/香港", "更新已触发");

    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw provider dialog");
    let rendered = buffer_compact_lines(terminal.backend().buffer()).join("\n");
    assert!(rendered.contains("Provider"));
    assert!(rendered.contains("远程Provider/香港"));
    assert!(rendered.contains("零时间Provider"));
    assert!(!rendered.contains("0001-01-01"));
    assert!(rendered.contains("更新已触发"));
    assert!(rendered.contains("流量"));
    assert_eq!(app.proxy_pane, ProxyPane::Nodes);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn provider_selection_keeps_stable_key_after_refresh() {
    let mut app = TuiApp {
        proxy_providers: vec![
            ProxyProviderRow {
                name: "A".into(),
                ..ProxyProviderRow::default()
            },
            ProxyProviderRow {
                name: "B".into(),
                ..ProxyProviderRow::default()
            },
        ],
        proxy_provider_index: 1,
        ..TuiApp::default()
    };
    app.remember_provider_selection(ProviderDialogKind::Proxy);
    app.apply_proxy_providers(vec![
        ProxyProviderRow {
            name: "C".into(),
            ..ProxyProviderRow::default()
        },
        ProxyProviderRow {
            name: "B".into(),
            ..ProxyProviderRow::default()
        },
    ]);

    assert_eq!(app.proxy_provider_index, 1);
    assert_eq!(
        app.selected_provider_name(ProviderDialogKind::Proxy).as_deref(),
        Some("B")
    );
}

#[test]
fn proxy_provider_rows_follow_desktop_visible_vehicle_types() {
    let providers: ProxyProvidersResponse = serde_json::from_value(json!({
        "providers": {
            "remote-http": {
                "name": "remote-http",
                "type": "Proxy",
                "vehicleType": "HTTP",
                "proxies": [{ "name": "HK" }]
            },
            "local-file": {
                "name": "local-file",
                "type": "Proxy",
                "vehicleType": "File",
                "proxies": []
            },
            "compatible-default": {
                "name": "compatible-default",
                "type": "Proxy",
                "vehicleType": "Compatible",
                "proxies": [{ "name": "DIRECT" }]
            },
            "missing-vehicle": {
                "name": "missing-vehicle",
                "type": "Proxy",
                "proxies": []
            }
        }
    }))
    .expect("proxy providers");

    let rows = proxy_providers_from_response(&providers);
    let names = rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>();
    assert_eq!(names, vec!["local-file", "remote-http"]);
}

#[tokio::test]
async fn proxy_focus_cycles_only_between_groups_and_nodes() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-proxy-focus-two-pane-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let mut app = TuiApp {
        view: View::Proxies,
        proxy_pane: ProxyPane::Groups,
        proxy_groups: vec![ProxyGroupRow {
            name: "GLOBAL".into(),
            now: "香港节点".into(),
            nodes: vec!["香港节点".into(), "日本节点".into()],
            offline: false,
        }],
        ..TuiApp::default()
    };

    assert!(!app.handle_key(KeyCode::Char('f'), &state).await.expect("focus nodes"));
    assert_eq!(app.proxy_pane, ProxyPane::Nodes);
    assert_eq!(app.status, "当前焦点：节点");
    assert!(!app.handle_key(KeyCode::Char('f'), &state).await.expect("focus groups"));
    assert_eq!(app.proxy_pane, ProxyPane::Groups);
    assert_eq!(app.status, "当前焦点：策略组");
    assert!(
        !app.handle_key(KeyCode::Char('f'), &state)
            .await
            .expect("focus nodes again")
    );
    assert_eq!(app.proxy_pane, ProxyPane::Nodes);

    let _ = std::fs::remove_dir_all(root);
}

fn buffer_marker_columns(buffer: &ratatui::buffer::Buffer, marker: &str) -> Vec<u16> {
    let area = buffer.area;
    let mut columns = Vec::new();
    for y in area.y..area.y + area.height {
        let mut compact = String::new();
        let mut byte_columns = Vec::new();
        for x in area.x..area.x + area.width {
            let Some(cell) = buffer.cell((x, y)) else {
                continue;
            };
            let symbol = cell.symbol();
            if symbol == " " {
                continue;
            }
            byte_columns.push((compact.len(), x));
            compact.push_str(symbol);
        }
        if let Some(offset) = compact.find(marker)
            && let Some((_, column)) = byte_columns.iter().find(|(start, _)| *start == offset)
        {
            columns.push(*column);
        }
    }
    columns
}

fn buffer_compact_lines(buffer: &ratatui::buffer::Buffer) -> Vec<String> {
    let area = buffer.area;
    let mut lines = Vec::new();
    for y in area.y..area.y + area.height {
        lines.push(buffer_compact_line(buffer, y));
    }
    lines
}

fn buffer_compact_line(buffer: &ratatui::buffer::Buffer, y: u16) -> String {
    let area = buffer.area;
    let mut compact = String::new();
    for x in area.x..area.x + area.width {
        let Some(cell) = buffer.cell((x, y)) else {
            continue;
        };
        let symbol = cell.symbol();
        if symbol != " " {
            compact.push_str(symbol);
        }
    }
    compact
}

fn buffer_marker_column_on_row(buffer: &ratatui::buffer::Buffer, row_marker: &str, marker: &str) -> u16 {
    let area = buffer.area;
    for y in area.y..area.y + area.height {
        let mut compact = String::new();
        let mut byte_columns = Vec::new();
        for x in area.x..area.x + area.width {
            let Some(cell) = buffer.cell((x, y)) else {
                continue;
            };
            let symbol = cell.symbol();
            if symbol == " " {
                continue;
            }
            byte_columns.push((compact.len(), x));
            compact.push_str(symbol);
        }
        if !compact.contains(row_marker) {
            continue;
        }
        let Some(offset) = compact.find(marker) else {
            continue;
        };
        if let Some((_, column)) = byte_columns.iter().find(|(start, _)| *start == offset) {
            return *column;
        }
    }
    unreachable!("missing marker {marker:?} on row {row_marker:?}")
}

fn buffer_marker_first_cell<'a>(buffer: &'a ratatui::buffer::Buffer, marker: &str) -> &'a ratatui::buffer::Cell {
    let area = buffer.area;
    for y in area.y..area.y + area.height {
        let mut compact = String::new();
        let mut byte_columns = Vec::new();
        for x in area.x..area.x + area.width {
            let Some(cell) = buffer.cell((x, y)) else {
                continue;
            };
            let symbol = cell.symbol();
            if symbol == " " {
                continue;
            }
            byte_columns.push((compact.len(), x));
            compact.push_str(symbol);
        }
        let Some(offset) = compact.find(marker) else {
            continue;
        };
        if let Some((_, column)) = byte_columns.iter().find(|(start, _)| *start == offset) {
            return buffer.cell((*column, y)).expect("marker cell");
        }
    }
    unreachable!("missing marker {marker:?}")
}

#[tokio::test]
async fn proxies_view_explains_empty_loaded_state() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-proxy-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let backend = TestBackend::new(120, 26);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let app = TuiApp {
        view: View::Proxies,
        ..TuiApp::default()
    };

    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("当前未加载策略组"));
    assert!(rendered.contains("CLI proxy groups"));
    assert!(rendered.contains("按 D"));
    assert!(rendered.contains("diagnose --json"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn proxies_view_shows_runtime_diagnose_details_when_empty() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-proxy-runtime-details-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let backend = TestBackend::new(150, 32);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let app = TuiApp {
        view: View::Proxies,
        diagnose_report: Some(actions::diagnose::DiagnoseReport {
            status: actions::diagnose::DiagnoseStatus::NeedsAttention,
            current_profile: Some(actions::diagnose::ProfileBrief {
                uid: Some("remote:formal".into()),
                name: Some("正式订阅".into()),
                profile_type: Some("remote".into()),
            }),
            kernel: KernelSnapshot {
                state: KernelState::Running,
                owner: KernelOwner::Detached,
                owner_detail: None,
                pid: Some(42),
                version: Some("test".into()),
                last_error: None,
                last_exit: None,
            },
            controller: actions::diagnose::ControllerProbe {
                health: ControllerHealth {
                    healthy: true,
                    version: Some("test".into()),
                    message: None,
                },
            },
            runtime: actions::diagnose::RuntimeProbe {
                readable: true,
                path: "/tmp/runtime.yaml".into(),
                proxies: 3,
                providers: 1,
                groups: 1,
                rules: 2,
                proxy_types: vec![
                    actions::diagnose::RuntimeTypeCount {
                        proxy_type: "vless".into(),
                        count: 2,
                    },
                    actions::diagnose::RuntimeTypeCount {
                        proxy_type: "hysteria2".into(),
                        count: 1,
                    },
                ],
                proxy_samples: vec!["香港 A".into(), "日本 B".into()],
                provider_samples: vec!["remote-provider".into()],
                group_samples: vec!["Proxy".into()],
                group_proxy_refs: 1,
                group_provider_refs: 1,
                error: None,
            },
            proxies: actions::diagnose::ProxyProbe {
                ready: false,
                entries: 0,
                groups: 0,
                nodes: 0,
                error: Some("controller empty".into()),
            },
            network: actions::diagnose::NetworkProbe {
                tun: crate::platform::TunDiagnostics {
                    platform: "linux".into(),
                    enabled: true,
                    can_enable: false,
                    checks: vec![crate::platform::TunCheck {
                        name: "privilege".into(),
                        ok: false,
                        message: "未找到 CAP_NET_ADMIN".into(),
                    }],
                    manual_action: Some("执行 tun off 和 core stop 恢复".into()),
                    message: "当前 Linux 环境不满足 TUN 开启条件".into(),
                },
                system_proxy_enabled: false,
                system_proxy: crate::platform::SystemProxyDiagnostics {
                    platform: "linux".into(),
                    endpoint: crate::platform::SystemProxyEndpoint {
                        host: "127.0.0.1".into(),
                        port: 7897,
                        bypass: "localhost,127.0.0.1".into(),
                    },
                    auto_apply_supported: true,
                    can_auto_apply: false,
                    checks: Vec::new(),
                    manual_action: None,
                    message: "系统代理需手动配置".into(),
                },
            },
            logs: actions::diagnose::LogProbe {
                recent: Vec::new(),
                warnings: 0,
                errors: 1,
                last_error: Some("Core 最近错误：load failed".into()),
            },
            subscription: None,
            recommendations: vec![
                "runtime 已有策略组但 controller 未加载".into(),
                "TUN 已开启但当前环境不满足基本条件：未找到 CAP_NET_ADMIN；处理建议：执行 tun off 和 core stop 恢复"
                    .into(),
            ],
        }),
        ..TuiApp::default()
    };

    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("最近诊断"));
    assert!(rendered.contains("建议 1"));
    assert!(rendered.contains("建议 2"));
    assert!(rendered.contains("TUN 已开启"));
    assert!(rendered.contains("tun off"));
    assert!(rendered.contains("core stop"));
    assert!(rendered.contains("runtime 配置"));
    assert!(rendered.contains("runtime 类型"));
    assert!(rendered.contains("vless=2"));
    assert!(rendered.contains("runtime 策略组样本"));
    assert!(rendered.contains("策略组引用"));
    assert!(rendered.contains("runtime Provider 样本"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn proxies_view_labels_runtime_preview_groups() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-runtime-preview-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let backend = TestBackend::new(140, 28);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let app = TuiApp {
        view: View::Proxies,
        proxy_groups: vec![ProxyGroupRow {
            name: "Proxy".into(),
            now: "未预选".into(),
            nodes: vec!["HK".into(), "Provider: remote".into()],
            offline: true,
        }],
        ..TuiApp::default()
    };

    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("runtime预选"));
    assert!(rendered.contains("未预选"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn diagnose_key_updates_status_and_proxy_empty_render() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-diagnose-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let backend = TestBackend::new(140, 28);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut app = TuiApp {
        view: View::Proxies,
        ..TuiApp::default()
    };

    assert!(!app.handle_key(KeyCode::Char('D'), &state).await.expect("diagnose"));
    assert!(app.diagnose_report.is_some());
    assert!(app.status.contains("诊断：阻塞"));
    assert!(app.status.contains("Profile=未选择"));
    assert!(app.status.contains("日志错误=0"));

    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("最近诊断"));
    assert!(rendered.contains("建议"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn export_diagnose_key_writes_snapshot_without_urls() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-diagnose-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let mut app = TuiApp {
        view: View::Proxies,
        ..TuiApp::default()
    };

    assert!(!app.handle_key(KeyCode::Char('E'), &state).await.expect("export"));

    assert!(app.status.contains("诊断快照已保存"));
    let (_, path) = app.status.split_once('：').expect("snapshot path in status");
    let content = std::fs::read_to_string(path).expect("snapshot");
    assert!(content.contains("\"runtime\""));
    assert!(!content.contains("https://"));
    assert!(!content.contains("http://"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn proxies_refresh_auto_populates_diagnose_when_empty() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-proxy-auto-diagnose-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let backend = TestBackend::new(140, 28);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut app = TuiApp {
        view: View::Proxies,
        ..TuiApp::default()
    };

    app.refresh(&state).await;

    assert!(app.diagnose_report.is_some());
    assert!(app.status.contains("诊断"));
    assert!(app.status.contains("策略组不可用") || app.status.contains("策略组为空"));

    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("最近诊断"));
    assert!(rendered.contains("建议"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn explicit_proxy_refresh_preserves_empty_diagnose_status() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-proxy-refresh-diagnose-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let mut app = TuiApp {
        view: View::Proxies,
        ..TuiApp::default()
    };

    app.refresh_now(&state).await;

    assert!(app.proxy_groups.is_empty());
    assert!(app.diagnose_report.is_some());
    assert!(app.status.contains("诊断"));
    assert!(!app.status.contains("已刷新代理"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn proxy_refresh_keeps_runtime_preview_status_when_provider_is_offline() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-proxy-runtime-status-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let runtime_path = state.config.read().await.paths.runtime_config.clone();
    tokio::fs::write(
        &runtime_path,
        r"proxies:
  - name: HK
    type: direct
proxy-groups:
  - name: Proxy
    type: select
    proxies:
      - HK
rules:
  - MATCH,Proxy
",
    )
    .await
    .expect("write runtime");
    let backend = TestBackend::new(140, 28);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut app = TuiApp {
        view: View::Proxies,
        ..TuiApp::default()
    };

    app.refresh(&state).await;

    assert!(app.proxy_groups.iter().any(|group| group.offline));
    assert!(app.status.contains("runtime 离线预览"));
    assert!(app.status.contains("可预选节点"));
    assert!(!app.status.contains("Provider 不可用"));
    assert!(app.kernel_snapshot.is_some());

    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("核心：已停止"));
    assert!(rendered.contains("runtime预选"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn proxy_refresh_preserves_runtime_preview_selection() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-proxy-runtime-selection-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let runtime_path = state.config.read().await.paths.runtime_config.clone();
    tokio::fs::write(
        &runtime_path,
        r"proxies:
  - name: HK
    type: direct
  - name: JP
    type: direct
  - name: US
    type: direct
proxy-groups:
  - name: 🚀节点选择
    type: select
    proxies:
      - HK
      - JP
  - name: 🇯🇵日本节点
    type: select
    proxies:
      - JP
  - name: 🇺🇸美国节点
    type: select
    proxies:
      - US
rules:
  - MATCH,🚀节点选择
",
    )
    .await
    .expect("write runtime");
    let mut app = TuiApp {
        view: View::Proxies,
        ..TuiApp::default()
    };

    app.refresh(&state).await;
    app.move_selection(2);
    app.last_refresh = None;
    app.refresh(&state).await;

    assert_eq!(
        app.selected_proxy_group().map(|group| group.name.as_str()),
        Some("🇺🇸美国节点")
    );
    assert_eq!(app.proxy_node_index, 0);

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn search_input_updates_active_filter() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-search-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let mut app = TuiApp {
        view: View::Rules,
        ..TuiApp::default()
    };

    assert!(!app.handle_key(KeyCode::Char('/'), &state).await.expect("filter"));
    assert!(!app.handle_key(KeyCode::Char('d'), &state).await.expect("d"));
    assert!(!app.handle_key(KeyCode::Char('n'), &state).await.expect("n"));
    assert!(!app.handle_key(KeyCode::Char('s'), &state).await.expect("s"));
    assert!(!app.handle_key(KeyCode::Enter, &state).await.expect("enter"));
    assert_eq!(app.rule_query, "dns");

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn logs_level_filter_and_clear_are_actionable() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-logs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let backend = TestBackend::new(120, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut app = TuiApp {
        view: View::Logs,
        logs: vec![
            "time=now level=info msg=ready".into(),
            "time=now level=error msg=failed".into(),
            "time=now level=warn msg=slow".into(),
        ],
        ..TuiApp::default()
    };

    assert_eq!(app.filtered_log_indices().len(), 3);
    assert!(!app.handle_key(KeyCode::Char('L'), &state).await.expect("level"));
    assert_eq!(app.log_level_filter, LogLevelFilter::Error);
    assert_eq!(app.filtered_log_indices(), vec![1]);
    assert!(app.status.contains("错误"));

    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("等级：错误"));
    assert!(rendered.contains("错误"));
    assert!(rendered.contains("failed"));
    assert!(!rendered.contains("level=info"));

    assert!(!app.handle_key(KeyCode::Char('x'), &state).await.expect("clear prompt"));
    assert!(app.confirm.is_some());
    assert!(!app.handle_key(KeyCode::Char('y'), &state).await.expect("confirm clear"));
    assert!(app.logs.is_empty());
    assert_eq!(app.log_index, 0);
    assert!(app.status.contains("已清空日志"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn logs_view_explains_disabled_core_log() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-logs-disabled-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let backend = TestBackend::new(120, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut settings = actions::config::settings(&state).await.expect("settings");
    settings.core_log_enabled = false;
    let app = TuiApp {
        view: View::Logs,
        settings: Some(settings),
        logs: Vec::new(),
        ..TuiApp::default()
    };

    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("核心日志"));
    assert!(rendered.contains("关闭"));
    assert!(rendered.contains("重新开启并重启 Core 后继续记录"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn logs_render_control_chars_safely_before_switching_views() {
    assert_eq!(
        terminal_safe_text("level=error msg=panic\nruntime.traceback\t\u{1b}[31mred"),
        "level=error msg=panic <换行> runtime.traceback  [31mred"
    );
    assert_eq!(
        terminal_safe_log_text("level=error url=https://example.invalid/sub?token=secret\nnext"),
        "level=error url=[链接] <换行> next"
    );

    let root = std::env::temp_dir().join(format!("clash-tui-tui-log-residual-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let backend = TestBackend::new(120, 28);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut app = TuiApp {
        view: View::Logs,
        logs: vec!["level=error msg=panic\nruntime.traceback\t\u{1b}[31mred".into()],
        ..TuiApp::default()
    };

    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw logs");
    let rendered_logs = format!("{:?}", terminal.backend().buffer());
    assert!(rendered_logs.contains("panic"));
    assert!(!rendered_logs.contains("runtime.traceback"));
    assert!(!rendered_logs.contains('\u{1b}'));

    app.logs = vec!["level=error url=https://example.invalid/sub?token=secret".into()];
    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw redacted logs");
    let rendered_logs = format!("{:?}", terminal.backend().buffer());
    assert!(rendered_logs.contains("[链接]"));
    assert!(!rendered_logs.contains("https://"));

    assert!(!app.handle_key(KeyCode::Enter, &state).await.expect("log detail"));
    assert!(app.detail.is_some());
    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw log detail");
    let rendered_detail = format!("{:?}", terminal.backend().buffer());
    assert!(rendered_detail.contains("日志详情"));
    assert!(rendered_detail.contains("[链接]"));
    assert!(!rendered_detail.contains("https://"));
    assert!(!app.handle_key(KeyCode::Esc, &state).await.expect("close detail"));

    app.view = View::Settings;
    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw settings");
    let rendered_settings = format!("{:?}", terminal.backend().buffer());
    assert!(rendered_settings.contains("设置不可用"));
    assert!(!rendered_settings.contains("runtime.traceback"));
    assert!(!rendered_settings.contains("<换行>"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn subscription_import_input_rejects_invalid_url_without_leaking_value() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-import-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let mut app = TuiApp {
        view: View::Profiles,
        ..TuiApp::default()
    };

    assert!(validate_subscription_url("https://example.invalid/sub").is_ok());
    assert!(validate_subscription_url("ftp://example.invalid/sub").is_err());
    assert!(!app.handle_key(KeyCode::Char('i'), &state).await.expect("import"));
    for ch in "ftp://example.invalid/sub".chars() {
        assert!(!app.handle_key(KeyCode::Char(ch), &state).await.expect("type"));
    }
    assert!(!app.handle_key(KeyCode::Enter, &state).await.expect("enter"));

    assert!(app.status.contains("http:// 或 https://"));
    assert!(!app.status.contains("example.invalid"));
    assert_eq!(
        sanitize_url_error("download failed https://example.invalid/sub?token=secret"),
        "download failed [订阅链接]"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn paste_event_appends_text_in_input_mode() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-paste-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let mut app = TuiApp {
        view: View::Profiles,
        ..TuiApp::default()
    };

    assert!(!app.handle_key(KeyCode::Char('i'), &state).await.expect("import"));
    assert!(!app.handle_paste("https://example.invalid/sub\n".into()));
    let input = app.input.as_ref().expect("input mode");
    assert_eq!(input.value, "https://example.invalid/sub");
    assert!(app.status.contains("已粘贴"));
    assert_eq!(normalize_pasted_text("a\r\nb\n"), "ab");

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn global_subscription_paste_prefills_import_input() {
    let mut app = TuiApp {
        view: View::Dashboard,
        ..TuiApp::default()
    };

    assert!(!app.handle_paste("https://example.invalid/sub?token=secret\n".into()));

    let input = app.input.as_ref().expect("import input");
    assert_eq!(app.view, View::Profiles);
    assert_eq!(input.target, InputTarget::ImportSubscriptionUrl);
    assert_eq!(input.value, "https://example.invalid/sub?token=secret");
    assert!(app.status.contains("已识别订阅链接"));
    assert!(!app.status.contains("secret"));
    assert_eq!(
        pasted_subscription_url(" https://example.invalid/sub\n").as_deref(),
        Some("https://example.invalid/sub")
    );
}

#[tokio::test]
async fn local_profile_import_input_reads_file_path() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-local-import-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create temp root");
    let profile_path = root.join("profile.yaml");
    std::fs::write(&profile_path, "proxies: []\nproxy-groups: []\nrules: []\n").expect("write profile");
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let mut app = TuiApp {
        view: View::Profiles,
        ..TuiApp::default()
    };

    assert!(!app.handle_key(KeyCode::Char('o'), &state).await.expect("local import"));
    for ch in profile_path.to_string_lossy().chars() {
        assert!(!app.handle_key(KeyCode::Char(ch), &state).await.expect("type path"));
    }
    assert!(!app.handle_key(KeyCode::Enter, &state).await.expect("enter"));
    assert!(app.status.contains("本地配置导入成功"));
    assert!(!app.profiles.is_empty());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn raw_tui_input_parser_handles_ssh_tty_keys_and_paste() {
    let mut pending = Vec::new();
    assert_eq!(
        tui_input_events_from_bytes(&mut pending, b"23q"),
        vec![
            TuiInputEvent::Key(KeyCode::Char('2')),
            TuiInputEvent::Key(KeyCode::Char('3')),
            TuiInputEvent::Key(KeyCode::Char('q')),
        ]
    );
    assert!(pending.is_empty());

    assert_eq!(
        tui_input_events_from_bytes(&mut pending, b"\x1b[A\x1b[B\x1b[5~\x03"),
        vec![
            TuiInputEvent::Key(KeyCode::Up),
            TuiInputEvent::Key(KeyCode::Down),
            TuiInputEvent::Key(KeyCode::PageUp),
            TuiInputEvent::Key(KeyCode::Esc),
        ]
    );
    assert!(pending.is_empty());

    let paste = b"\x1b[200~https://example.invalid/sub?token=secret\n\x1b[201~\r";
    assert_eq!(
        tui_input_events_from_bytes(&mut pending, paste),
        vec![
            TuiInputEvent::Paste("https://example.invalid/sub?token=secret\n".into()),
            TuiInputEvent::Key(KeyCode::Enter),
        ]
    );
    assert!(pending.is_empty());
}

#[test]
fn raw_tui_input_parser_keeps_incomplete_utf8_until_complete() {
    let mut pending = Vec::new();
    let first = tui_input_events_from_bytes(&mut pending, &[0xE4, 0xB8]);
    assert!(first.is_empty());
    assert_eq!(
        tui_input_events_from_bytes(&mut pending, &[0xAD]),
        vec![TuiInputEvent::Key(KeyCode::Char('中'))]
    );
    assert!(pending.is_empty());
}

#[test]
fn input_trace_does_not_log_typed_or_pasted_secrets() {
    let key_line = tui_input_event_trace_line(&TuiInputEvent::Key(KeyCode::Char('s')));
    assert!(key_line.contains("code=char"));
    assert!(!key_line.contains("code=s"));

    let paste_line =
        tui_input_event_trace_line(&TuiInputEvent::Paste("https://example.invalid/sub?token=secret".into()));
    assert!(paste_line.starts_with("paste len="));
    assert!(!paste_line.contains("example.invalid"));
    assert!(!paste_line.contains("token"));
}

#[test]
fn pinned_import_status_survives_kernel_events() {
    let (sender, mut receiver) = tokio::sync::broadcast::channel(4);
    let mut app = TuiApp {
        status: "订阅导入并激活成功（direct），Core 已启动，策略组 4 个，节点 164 个".into(),
        ..TuiApp::default()
    };
    app.pin_status(Duration::from_secs(10));

    sender
        .send(ClashTuiEvent {
            id: 1,
            timestamp: 0,
            payload: ClashTuiEventPayload::KernelStateChanged {
                kernel: KernelSnapshot {
                    state: KernelState::Running,
                    owner: KernelOwner::Detached,
                    owner_detail: None,
                    pid: Some(42),
                    version: Some("test".into()),
                    last_error: None,
                    last_exit: None,
                },
            },
        })
        .expect("send kernel event");
    drain_job_events(&mut app, &mut receiver);
    assert!(app.status.contains("订阅导入并激活成功"));
    assert!(!app.status.contains("核心状态"));

    app.clear_status_pin();
    sender
        .send(ClashTuiEvent {
            id: 2,
            timestamp: 0,
            payload: ClashTuiEventPayload::KernelStateChanged {
                kernel: KernelSnapshot::stopped(),
            },
        })
        .expect("send kernel event");
    drain_job_events(&mut app, &mut receiver);
    assert_eq!(app.status, "核心状态：已停止");
}

#[test]
fn pinned_action_status_survives_refresh_and_job_events() {
    let (sender, mut receiver) = tokio::sync::broadcast::channel(8);
    let mut app = TuiApp::default();
    app.set_important_status("已为 GLOBAL 选择 香港节点");

    app.set_refresh_status("规则不可用：controller timeout");
    assert_eq!(app.status, "已为 GLOBAL 选择 香港节点");

    sender
        .send(ClashTuiEvent {
            id: 1,
            timestamp: 0,
            payload: ClashTuiEventPayload::JobCreated {
                job: JobRecord {
                    id: "job-1".into(),
                    kind: "profile-update".into(),
                    name: "更新订阅".into(),
                    target: None,
                    status: JobStatus::Running,
                    message: None,
                    error: None,
                    result: None,
                    created_at: 1,
                    updated_at: 1,
                    finished_at: None,
                },
            },
        })
        .expect("send job event");
    drain_job_events(&mut app, &mut receiver);
    assert_eq!(app.status, "已为 GLOBAL 选择 香港节点");

    app.clear_status_pin();
    app.set_refresh_status("规则不可用：controller timeout");
    assert_eq!(app.status, "规则不可用：controller timeout");
}

#[test]
fn current_profile_update_job_event_clears_proxy_runtime_state() {
    let (sender, mut receiver) = tokio::sync::broadcast::channel(4);
    let mut app = TuiApp {
        profiles_current: Some("Rcurrent".into()),
        proxy_groups: vec![ProxyGroupRow {
            name: "GLOBAL".into(),
            now: "OldNode".into(),
            nodes: vec!["OldNode".into()],
            offline: false,
        }],
        last_refresh: Some(Instant::now()),
        ..TuiApp::default()
    };

    sender
        .send(ClashTuiEvent {
            id: 1,
            timestamp: 0,
            payload: ClashTuiEventPayload::JobUpdated {
                job: JobRecord {
                    id: "job-1".into(),
                    kind: "profile-update".into(),
                    name: "Update remote profile Rcurrent".into(),
                    target: Some("Rcurrent".into()),
                    status: JobStatus::Succeeded,
                    message: Some("profile updated; runtime refreshed".into()),
                    error: None,
                    result: Some(serde_json::json!({
                        "uid": "Rcurrent",
                        "current": "Rcurrent",
                        "currentProfile": true,
                        "runtimePath": "/tmp/runtime.yaml",
                        "runtimeValidated": true,
                        "runtimeReloaded": false,
                    })),
                    created_at: 1,
                    updated_at: 2,
                    finished_at: Some(3),
                },
            },
        })
        .expect("send job event");

    drain_job_events(&mut app, &mut receiver);

    assert!(app.proxy_groups.is_empty());
    assert!(app.last_refresh.is_none());
    assert!(app.status.contains("当前订阅已更新"));
}

#[test]
fn non_current_profile_update_job_event_keeps_proxy_runtime_state() {
    let (sender, mut receiver) = tokio::sync::broadcast::channel(4);
    let mut app = TuiApp {
        profiles_current: Some("Rcurrent".into()),
        proxy_groups: vec![ProxyGroupRow {
            name: "GLOBAL".into(),
            now: "OldNode".into(),
            nodes: vec!["OldNode".into()],
            offline: false,
        }],
        ..TuiApp::default()
    };

    sender
        .send(ClashTuiEvent {
            id: 1,
            timestamp: 0,
            payload: ClashTuiEventPayload::JobUpdated {
                job: JobRecord {
                    id: "job-1".into(),
                    kind: "profile-update".into(),
                    name: "Update remote profile Rother".into(),
                    target: Some("Rother".into()),
                    status: JobStatus::Succeeded,
                    message: Some("profile updated".into()),
                    error: None,
                    result: Some(serde_json::json!({
                        "uid": "Rother",
                        "current": "Rcurrent",
                        "currentProfile": false,
                    })),
                    created_at: 1,
                    updated_at: 2,
                    finished_at: Some(3),
                },
            },
        })
        .expect("send job event");

    drain_job_events(&mut app, &mut receiver);

    assert_eq!(app.proxy_groups.len(), 1);
    assert!(!app.status.contains("当前订阅已更新"));
}

#[tokio::test]
async fn status_history_modal_shows_full_sanitized_messages() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-status-history-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let backend = TestBackend::new(120, 28);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut app = TuiApp::default();

    app.set_important_status("订阅导入失败：https://example.invalid/sub?token=secret\n下一行");
    app.clear_status_pin();
    app.set_refresh_status("规则不可用：controller timeout");

    assert!(!app.handle_key(KeyCode::Char('n'), &state).await.expect("history"));
    let detail = app.detail.as_ref().expect("status history detail");
    assert_eq!(detail.title, "消息历史");
    let joined = detail.lines.join("\n");
    assert!(joined.contains("当前状态：规则不可用：controller timeout"));
    assert!(joined.contains("订阅导入失败：[链接]"));
    assert!(joined.contains("<换行>"));
    assert!(!joined.contains("https://"));
    assert!(!joined.contains("token=secret"));

    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw status history");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("消息历史"));
    assert!(rendered.contains("规则不可用"));
    assert!(!rendered.contains("https://"));
    assert!(!rendered.contains("token=secret"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn footer_lines_are_sanitized_and_width_bound() {
    let app = TuiApp {
        view: View::Proxies,
        status: "错误：https://example.invalid/sub?token=secret\n下一行".into(),
        proxy_query: "香港 https://example.invalid/query?token=secret 很长很长很长".into(),
        ..TuiApp::default()
    };

    let lines = super::footer_line_strings(&app, 40);

    assert!(
        lines
            .iter()
            .all(|line| crate::tui::views::layout::display_width(line) <= 40)
    );
    let rendered = lines.join("\n");
    assert!(rendered.contains("[链接]"));
    assert!(rendered.contains("<换行>") || rendered.contains("..."));
    assert!(!rendered.contains("https://"));
    assert!(!rendered.contains("token=secret"));

    let app = TuiApp {
        status: "订阅链接必须以 http:// 或 https:// 开头".into(),
        ..TuiApp::default()
    };
    let lines = super::footer_line_strings(&app, 80);
    assert!(lines[0].contains("http://"));
    assert!(lines[0].contains("https://"));
}

#[test]
fn footer_help_compacts_without_partial_key_segments() {
    let app = TuiApp::default();

    let compact = super::footer_line_strings(&app, 70)[2].clone();
    assert!(crate::tui::views::layout::display_width(&compact) <= 70);
    assert!(compact.contains("…更多"));
    assert!(compact.contains("? 帮助"));
    assert!(!compact.contains("..."));
    assert!(!compact.contains("系统..."));

    let wide = super::footer_line_strings(&app, 140)[2].clone();
    assert!(crate::tui::views::layout::display_width(&wide) <= 140);
    assert!(wide.contains("P 系统代理"));
    assert!(wide.contains("T TUN"));
}

#[tokio::test]
async fn raw_detail_modal_redacts_urls_and_control_chars() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-detail-redact-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let backend = TestBackend::new(90, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let app = TuiApp {
        detail: Some(DetailState {
            title: "原始详情".into(),
            lines: vec!["失败 https://example.invalid/sub?token=secret\n下一行\t含制表".into()],
        }),
        ..TuiApp::default()
    };

    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw raw detail");
    let rendered = format!("{:?}", terminal.backend().buffer());

    assert!(rendered.contains("原始详情"));
    assert!(rendered.contains("[链接]"));
    assert!(rendered.contains("<换行>"));
    assert!(!rendered.contains("https://"));
    assert!(!rendered.contains("token=secret"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn status_history_is_bounded_and_deduplicated() {
    let mut app = TuiApp::default();
    app.set_refresh_status("重复状态");
    app.set_refresh_status("重复状态");
    assert_eq!(app.status_history.len(), 1);

    for index in 0..(super::STATUS_HISTORY_LIMIT + 5) {
        app.set_refresh_status(format!("状态 {index}"));
    }
    assert_eq!(app.status_history.len(), super::STATUS_HISTORY_LIMIT);
    assert!(!app.status_history.iter().any(|message| message == "状态 0"));
    assert_eq!(app.status_history.back().map(String::as_str), Some("状态 24"));
}

#[test]
fn user_visible_status_helpers_record_history() {
    let mut app = TuiApp::default();

    app.start_subscription_import();
    assert_eq!(
        app.status_history.back().map(String::as_str),
        Some("请粘贴订阅链接，按 Enter 导入")
    );

    app.view = View::Dashboard;
    app.start_search();
    assert_eq!(
        app.status_history.back().map(String::as_str),
        Some("当前页面不支持过滤")
    );

    app.view = View::Jobs;
    app.open_selected_job_detail();
    assert_eq!(app.status_history.back().map(String::as_str), Some("未选择任务"));

    app.status = "旧状态".into();
    app.open_status_history();
    assert!(app.status_history.iter().any(|message| message == "旧状态"));
    assert_eq!(app.status_history.back().map(String::as_str), Some("正在查看消息历史"));
}

#[test]
fn apply_profiles_clears_proxy_runtime_state_when_current_changes() {
    let mut app = TuiApp {
        profiles_current: Some("Rold".into()),
        proxy_groups: vec![ProxyGroupRow {
            name: "GLOBAL".into(),
            now: "OldNode".into(),
            nodes: vec!["OldNode".into()],
            offline: false,
        }],
        proxy_group_index: 3,
        proxy_node_index: 4,
        proxy_node_meta: BTreeMap::from([(
            "OldNode".into(),
            ProxyNodeMeta {
                proxy_type: "Vless".into(),
                delay_ms: Some(88),
                alive: Some(true),
            },
        )]),
        proxy_group_selection_key: Some("GLOBAL".into()),
        proxy_node_selection_key: Some("OldNode".into()),
        proxy_user_selection_at: Some(Instant::now()),
        proxy_providers: vec![ProxyProviderRow {
            name: "OldProvider".into(),
            ..ProxyProviderRow::default()
        }],
        proxy_provider_selection_key: Some("OldProvider".into()),
        rule_providers: vec![RuleProviderRow {
            name: "OldRuleProvider".into(),
            ..RuleProviderRow::default()
        }],
        rule_provider_selection_key: Some("OldRuleProvider".into()),
        provider_dialog: Some(ProviderDialogKind::Proxy),
        proxy_pane: ProxyPane::Nodes,
        dashboard_proxy_popup: DashboardProxyPopup::Nodes,
        rules: vec![RuleEntry {
            r#type: Some("MATCH".into()),
            proxy: Some("OldNode".into()),
            ..RuleEntry::default()
        }],
        rule_index: 2,
        connections: vec![sample_connection()],
        connection_index: 1,
        ..TuiApp::default()
    };

    app.apply_profiles(IProfiles {
        current: Some("Rnew".into()),
        items: Some(vec![PrfItem {
            uid: Some("Rnew".into()),
            itype: Some("remote".into()),
            ..PrfItem::default()
        }]),
    });

    assert_eq!(app.profiles_current.as_deref(), Some("Rnew"));
    assert!(app.proxy_groups.is_empty());
    assert!(app.proxy_node_meta.is_empty());
    assert!(app.proxy_providers.is_empty());
    assert!(app.rule_providers.is_empty());
    assert!(app.provider_dialog.is_none());
    assert!(app.proxy_group_selection_key.is_none());
    assert!(app.proxy_node_selection_key.is_none());
    assert!(app.proxy_provider_selection_key.is_none());
    assert!(app.rule_provider_selection_key.is_none());
    assert!(app.proxy_user_selection_at.is_none());
    assert_eq!(app.proxy_group_index, 0);
    assert_eq!(app.proxy_node_index, 0);
    assert_eq!(app.proxy_pane, ProxyPane::Groups);
    assert_eq!(app.dashboard_proxy_popup, DashboardProxyPopup::None);
    assert!(app.rules.is_empty());
    assert!(app.connections.is_empty());
}

#[test]
fn apply_profiles_keeps_proxy_runtime_state_when_current_is_unchanged() {
    let mut app = TuiApp {
        profiles_current: Some("Rcurrent".into()),
        proxy_groups: vec![ProxyGroupRow {
            name: "GLOBAL".into(),
            now: "Node".into(),
            nodes: vec!["Node".into()],
            offline: false,
        }],
        ..TuiApp::default()
    };

    app.apply_profiles(IProfiles {
        current: Some("Rcurrent".into()),
        items: Some(vec![PrfItem {
            uid: Some("Rcurrent".into()),
            itype: Some("remote".into()),
            ..PrfItem::default()
        }]),
    });

    assert_eq!(app.proxy_groups.len(), 1);
}

#[test]
fn profile_import_focuses_unfiltered_proxy_groups() {
    let mut app = TuiApp {
        view: View::Profiles,
        proxy_pane: ProxyPane::Groups,
        proxy_query: "香港".into(),
        proxy_group_index: 9,
        proxy_node_index: 8,
        ..TuiApp::default()
    };

    app.focus_proxy_groups_after_import();

    assert_eq!(app.view, View::Proxies);
    assert_eq!(app.proxy_pane, ProxyPane::Nodes);
    assert!(app.proxy_query.is_empty());
    assert_eq!(app.proxy_group_index, 0);
    assert_eq!(app.proxy_node_index, 0);
}

#[tokio::test]
async fn entering_proxy_view_focuses_and_renders_current_node() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-enter-proxy-current-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let current_node = "美国节点 18";
    let nodes = (0..30).map(|index| format!("美国节点 {index:02}")).collect::<Vec<_>>();
    let mut app = TuiApp {
        view: View::Dashboard,
        proxy_pane: ProxyPane::Groups,
        proxy_groups: vec![ProxyGroupRow {
            name: "GLOBAL".into(),
            now: current_node.into(),
            nodes,
            offline: false,
        }],
        ..TuiApp::default()
    };

    app.set_view(View::Proxies);

    assert_eq!(app.proxy_pane, ProxyPane::Nodes);
    assert_eq!(app.selected_proxy_node_name().as_deref(), Some(current_node));
    let backend = TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw current node");
    let buffer = terminal.backend().buffer();
    let rendered = format!("{buffer:?}");
    let compact_lines = buffer_compact_lines(buffer);
    assert!(rendered.contains("代理组"));
    assert!(rendered.contains("节点"));
    assert!(rendered.contains("├代理组"));
    assert!(rendered.contains("┬节点"));
    assert!(
        compact_lines
            .iter()
            .any(|line| line.starts_with("├代理组") && line.contains("┬节点")),
        "proxy split title should share the outer border without an extra left vertical line"
    );
    assert!(
        compact_lines.iter().all(|line| !line.contains("│├代理组")),
        "proxy split title must not render an extra left border before the title"
    );
    assert!(
        compact_lines
            .iter()
            .any(|line| line.starts_with("└") && line.contains("┴") && line.ends_with("┘")),
        "proxy split divider should connect to the bottom border"
    );
    assert!(rendered.contains(current_node));
    assert!(rendered.contains("*"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn offline_runtime_preview_selection_is_preselected() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-runtime-preview-select-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    state
        .store
        .import_local_profile(&LocalProfileImport {
            uid: Some("L001".into()),
            name: Some("Demo".into()),
            file_data: "proxies: []\nproxy-groups: []\nrules: []\n".into(),
        })
        .await
        .expect("profile");
    let mut app = TuiApp {
        view: View::Proxies,
        proxy_pane: ProxyPane::Nodes,
        proxy_groups: vec![ProxyGroupRow {
            name: "Proxy".into(),
            now: "未预选".into(),
            nodes: vec!["HK".into()],
            offline: true,
        }],
        ..TuiApp::default()
    };

    assert!(!app.handle_key(KeyCode::Enter, &state).await.expect("select"));
    assert!(app.status.contains("已预选 Proxy -> HK"));
    assert_eq!(app.selected_proxy_group().map(|group| group.now.as_str()), Some("HK"));
    let profiles = state.store.load_profiles().await.expect("profiles");
    let selected = profiles
        .get_item("L001")
        .expect("profile")
        .selected
        .as_ref()
        .expect("selected")
        .clone();
    assert_eq!(selected[0].name.as_deref(), Some("Proxy"));
    assert_eq!(selected[0].now.as_deref(), Some("HK"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn proxy_delay_test_uses_cursor_node_not_active_node() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-node-delay-cursor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let mut app = TuiApp {
        view: View::Proxies,
        proxy_pane: ProxyPane::Nodes,
        proxy_node_index: 1,
        proxy_groups: vec![ProxyGroupRow {
            name: "GLOBAL".into(),
            now: "香港节点".into(),
            nodes: vec!["香港节点".into(), "日本节点".into()],
            offline: false,
        }],
        ..TuiApp::default()
    };

    assert!(!app.handle_key(KeyCode::Char('t'), &state).await.expect("test delay"));
    assert!(app.status.contains("测速 日本节点 失败"));
    assert!(!app.status.contains("测速 香港节点"));
    assert_eq!(app.proxy_node_selection_key.as_deref(), Some("日本节点"));
    assert_eq!(
        app.selected_proxy_group().map(|group| group.now.as_str()),
        Some("香港节点")
    );

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn proxy_nodes_focus_current_and_filter_node_list() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-node-filter-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let backend = TestBackend::new(120, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut app = TuiApp {
        view: View::Proxies,
        proxy_groups: vec![ProxyGroupRow {
            name: "自动选择".into(),
            now: "日本节点".into(),
            nodes: vec!["香港节点".into(), "日本节点".into(), "新加坡节点".into()],
            offline: false,
        }],
        ..TuiApp::default()
    };

    assert!(!app.handle_key(KeyCode::Enter, &state).await.expect("enter group"));
    assert_eq!(app.proxy_pane, ProxyPane::Nodes);
    assert_eq!(app.proxy_node_index, 1);
    assert!(app.status.contains("已定位策略组：自动选择"));

    app.proxy_query = "香港".into();
    app.clamp_selections();
    assert_eq!(app.filtered_proxy_node_indices(), vec![0]);
    assert_eq!(app.proxy_node_index, 0);

    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("代理组"));
    assert!(rendered.contains("节点"));
    assert!(rendered.contains("当前"));
    assert!(!rendered.contains("类型"));
    assert!(rendered.contains("香港节点"));

    app.proxy_query = "不存在".into();
    app.clamp_selections();
    assert!(app.filtered_proxy_node_indices().is_empty());
    let backend = TestBackend::new(120, 24);
    let mut terminal = Terminal::new(backend).expect("terminal empty");
    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw empty");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("没有匹配当前过滤条件的节点"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn proxy_selection_restore_prefers_remembered_node_after_refresh() {
    let mut app = TuiApp {
        view: View::Proxies,
        proxy_pane: ProxyPane::Nodes,
        proxy_group_index: 0,
        proxy_node_index: 0,
        proxy_group_selection_key: Some("自动选择".into()),
        proxy_node_selection_key: Some("美国节点".into()),
        proxy_groups: vec![ProxyGroupRow {
            name: "自动选择".into(),
            now: "香港节点".into(),
            nodes: vec!["香港节点".into(), "日本节点".into(), "美国节点".into()],
            offline: false,
        }],
        ..TuiApp::default()
    };

    app.clamp_selections();

    assert_eq!(app.proxy_node_index, 2);

    app.apply_proxy_groups(vec![ProxyGroupRow {
        name: "自动选择".into(),
        now: "香港节点".into(),
        nodes: vec!["香港节点".into(), "日本节点".into(), "美国节点".into()],
        offline: false,
    }]);

    assert_eq!(app.proxy_node_index, 2);
    assert_eq!(app.proxy_node_selection_key.as_deref(), Some("美国节点"));
}

#[test]
fn proxy_node_selection_survives_repeated_refresh_after_scrolling() {
    let nodes = (0..12)
        .map(|index| format!("🇺🇸美国节点 {index:02}"))
        .collect::<Vec<_>>();
    let groups = vec![ProxyGroupRow {
        name: "🚀节点选择".into(),
        now: "离线预览".into(),
        nodes,
        offline: true,
    }];
    let mut app = TuiApp {
        view: View::Proxies,
        proxy_pane: ProxyPane::Nodes,
        proxy_groups: groups.clone(),
        ..TuiApp::default()
    };

    for _ in 0..7 {
        app.move_selection(1);
    }
    assert_eq!(app.selected_proxy_node_name().as_deref(), Some("🇺🇸美国节点 07"));
    assert_eq!(app.proxy_node_selection_key.as_deref(), Some("🇺🇸美国节点 07"));

    for _ in 0..5 {
        app.apply_proxy_groups(groups.clone());
        assert_eq!(app.selected_proxy_node_name().as_deref(), Some("🇺🇸美国节点 07"));
        assert_eq!(app.proxy_node_selection_key.as_deref(), Some("🇺🇸美国节点 07"));
    }
}

#[test]
fn proxy_node_refresh_prefers_recent_user_selection_over_stale_index() {
    let nodes = (0..12)
        .map(|index| format!("🇺🇸美国节点 {index:02}"))
        .collect::<Vec<_>>();
    let groups = vec![ProxyGroupRow {
        name: "🚀节点选择".into(),
        now: "🇺🇸美国节点 00".into(),
        nodes,
        offline: true,
    }];
    let mut app = TuiApp {
        view: View::Proxies,
        proxy_pane: ProxyPane::Nodes,
        proxy_group_index: 0,
        proxy_node_index: 0,
        proxy_group_selection_key: Some("🚀节点选择".into()),
        proxy_node_selection_key: Some("🇺🇸美国节点 07".into()),
        proxy_user_selection_at: Some(Instant::now()),
        proxy_groups: groups.clone(),
        ..TuiApp::default()
    };

    app.apply_proxy_groups(groups);

    assert_eq!(app.proxy_node_index, 7);
    assert_eq!(app.selected_proxy_node_name().as_deref(), Some("🇺🇸美国节点 07"));
    assert_eq!(app.proxy_node_selection_key.as_deref(), Some("🇺🇸美国节点 07"));
}

#[test]
fn proxy_node_user_selection_does_not_expire_during_repeated_refresh() {
    let nodes = (0..12)
        .map(|index| format!("🇺🇸美国节点 {index:02}"))
        .collect::<Vec<_>>();
    let groups = vec![ProxyGroupRow {
        name: "🚀节点选择".into(),
        now: "🇺🇸美国节点 00".into(),
        nodes,
        offline: false,
    }];
    let mut app = TuiApp {
        view: View::Proxies,
        proxy_pane: ProxyPane::Nodes,
        proxy_group_index: 0,
        proxy_node_index: 0,
        proxy_group_selection_key: Some("🚀节点选择".into()),
        proxy_node_selection_key: Some("🇺🇸美国节点 07".into()),
        proxy_user_selection_at: Some(Instant::now() - Duration::from_secs(60)),
        proxy_groups: groups.clone(),
        ..TuiApp::default()
    };

    for _ in 0..5 {
        app.apply_proxy_groups(groups.clone());
        assert_eq!(app.proxy_node_index, 7);
        assert_eq!(app.selected_proxy_node_name().as_deref(), Some("🇺🇸美国节点 07"));
        assert_eq!(app.proxy_node_selection_key.as_deref(), Some("🇺🇸美国节点 07"));
    }
}

#[tokio::test]
async fn proxy_nodes_show_delay_status_and_filter_hidden_metadata() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-node-meta-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let backend = TestBackend::new(150, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut proxy_node_meta = BTreeMap::new();
    proxy_node_meta.insert(
        "香港节点".into(),
        ProxyNodeMeta {
            proxy_type: "Vless".into(),
            delay_ms: Some(80),
            alive: Some(true),
        },
    );
    proxy_node_meta.insert(
        "日本节点".into(),
        ProxyNodeMeta {
            proxy_type: "Trojan".into(),
            delay_ms: Some(320),
            alive: Some(false),
        },
    );
    let mut app = TuiApp {
        view: View::Proxies,
        proxy_pane: ProxyPane::Nodes,
        proxy_groups: vec![ProxyGroupRow {
            name: "自动选择".into(),
            now: "香港节点".into(),
            nodes: vec!["香港节点".into(), "日本节点".into()],
            offline: false,
        }],
        proxy_node_meta,
        ..TuiApp::default()
    };

    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw node meta");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(!rendered.contains("类型"));
    assert!(rendered.contains("延迟"));
    assert!(rendered.contains("状态"));
    assert!(!rendered.contains("Vless"));
    assert!(!rendered.contains("Trojan"));
    assert!(rendered.contains("80ms"));
    assert!(rendered.contains("可用"));
    assert!(rendered.contains("不可用"));

    app.proxy_query = "320".into();
    assert_eq!(app.filtered_proxy_node_indices(), vec![1]);
    app.proxy_query = "不可用".into();
    assert_eq!(app.filtered_proxy_node_indices(), vec![1]);
    app.proxy_query = "vless".into();
    assert_eq!(app.filtered_proxy_node_indices(), vec![0]);

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn proxy_nodes_strip_icons_and_align_metadata_columns() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-node-align-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let mut proxy_node_meta = BTreeMap::new();
    proxy_node_meta.insert(
        "🇭🇰香港节点".into(),
        ProxyNodeMeta {
            proxy_type: "Vless".into(),
            delay_ms: Some(80),
            alive: Some(true),
        },
    );
    proxy_node_meta.insert(
        "🚀美国节点".into(),
        ProxyNodeMeta {
            proxy_type: "Vless".into(),
            delay_ms: Some(120),
            alive: Some(true),
        },
    );
    proxy_node_meta.insert(
        "剩余流量：3.68 TB".into(),
        ProxyNodeMeta {
            proxy_type: "Vless".into(),
            delay_ms: Some(0),
            alive: Some(true),
        },
    );
    proxy_node_meta.insert(
        "服务号：机场通知".into(),
        ProxyNodeMeta {
            proxy_type: "Vless".into(),
            delay_ms: Some(168),
            alive: Some(true),
        },
    );
    let app = TuiApp {
        view: View::Proxies,
        proxy_pane: ProxyPane::Nodes,
        proxy_groups: vec![ProxyGroupRow {
            name: "🚀节点选择".into(),
            now: "🇭🇰香港节点".into(),
            nodes: vec![
                "🇭🇰香港节点".into(),
                "🚀美国节点".into(),
                "剩余流量：3.68 TB".into(),
                "服务号：机场通知".into(),
            ],
            offline: false,
        }],
        proxy_node_meta,
        ..TuiApp::default()
    };

    let backend = TestBackend::new(150, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw node alignment");
    let buffer = terminal.backend().buffer();
    let rendered = format!("{buffer:?}");
    assert!(!rendered.contains("🇭🇰"));
    assert!(!rendered.contains("🚀"));
    assert!(rendered.contains("香港节点"));
    assert!(rendered.contains("美国节点"));
    assert!(rendered.contains("剩余流量"));
    assert!(rendered.contains("服务号"));
    assert!(rendered.contains("当前"));
    assert!(rendered.contains("节点"));
    assert!(rendered.contains("延迟"));
    assert!(rendered.contains("状态"));
    assert!(!rendered.contains("类型"));
    assert!(!rendered.contains("Vless"));
    let status_columns = buffer_marker_columns(buffer, "可用");
    assert_eq!(status_columns.len(), 4);
    assert!(status_columns.windows(2).all(|pair| pair[0] == pair[1]));
    assert_eq!(
        buffer_marker_column_on_row(buffer, "剩余流量：3.68TB", "Timeout"),
        buffer_marker_column_on_row(buffer, "服务号：机场通知", "168ms")
    );
    assert_eq!(
        buffer_marker_column_on_row(buffer, "剩余流量：3.68TB", "可用"),
        buffer_marker_column_on_row(buffer, "服务号：机场通知", "可用")
    );
    let node_column = buffer_marker_column_on_row(buffer, "香港节点", "80ms");
    let delay_column = buffer_marker_column_on_row(buffer, "剩余流量：3.68TB", "Timeout");
    assert!(
        delay_column.saturating_sub(node_column) <= 70,
        "proxy node metadata columns should stay close to the node name on wide terminals"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn proxy_nodes_cycle_sort_by_latency_and_alive_metadata() {
    let root = std::env::temp_dir().join(format!("clash-tui-tui-node-sort-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let state = AppState::initialize(
        ClashTuiOptions::new(Some(root.clone()), Some(root.join("resources")), None, 300).expect("options"),
    )
    .await
    .expect("state");
    let mut proxy_node_meta = BTreeMap::new();
    proxy_node_meta.insert(
        "慢速不可用".into(),
        ProxyNodeMeta {
            proxy_type: "Trojan".into(),
            delay_ms: Some(420),
            alive: Some(false),
        },
    );
    proxy_node_meta.insert(
        "未知节点".into(),
        ProxyNodeMeta {
            proxy_type: "Vless".into(),
            delay_ms: None,
            alive: None,
        },
    );
    proxy_node_meta.insert(
        "快速可用".into(),
        ProxyNodeMeta {
            proxy_type: "Vless".into(),
            delay_ms: Some(40),
            alive: Some(true),
        },
    );
    proxy_node_meta.insert(
        "中速可用".into(),
        ProxyNodeMeta {
            proxy_type: "Shadowsocks".into(),
            delay_ms: Some(160),
            alive: Some(true),
        },
    );
    let mut app = TuiApp {
        view: View::Proxies,
        proxy_pane: ProxyPane::Groups,
        proxy_groups: vec![ProxyGroupRow {
            name: "自动选择".into(),
            now: "快速可用".into(),
            nodes: vec![
                "慢速不可用".into(),
                "未知节点".into(),
                "快速可用".into(),
                "中速可用".into(),
            ],
            offline: false,
        }],
        proxy_node_meta,
        ..TuiApp::default()
    };

    assert_eq!(app.filtered_proxy_node_indices(), vec![0, 1, 2, 3]);
    assert!(!app.handle_key(KeyCode::Char('S'), &state).await.expect("sort latency"));
    assert_eq!(app.proxy_pane, ProxyPane::Nodes);
    assert_eq!(app.proxy_node_sort, ProxyNodeSort::Latency);
    assert_eq!(app.filtered_proxy_node_indices(), vec![2, 3, 0, 1]);
    assert_eq!(app.proxy_node_index, 2);
    assert!(app.status.contains("延迟优先"));

    app.proxy_query = "vless".into();
    assert_eq!(app.filtered_proxy_node_indices(), vec![2, 1]);
    assert!(!app.handle_key(KeyCode::Char('S'), &state).await.expect("sort alive"));
    assert_eq!(app.proxy_node_sort, ProxyNodeSort::Alive);
    assert_eq!(app.filtered_proxy_node_indices(), vec![2, 1]);
    assert!(app.status.contains("可用优先"));
    assert!(
        !app.handle_key(KeyCode::Char('S'), &state)
            .await
            .expect("sort subscription")
    );
    assert_eq!(app.proxy_node_sort, ProxyNodeSort::Subscription);
    assert_eq!(app.filtered_proxy_node_indices(), vec![1, 2]);

    let backend = TestBackend::new(160, 24);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| render(frame.area(), frame.buffer_mut(), &app, &state))
        .expect("draw node sort");
    let rendered = format!("{:?}", terminal.backend().buffer());
    assert!(rendered.contains("排序：订阅顺序"));
    assert!(rendered.contains("S 节点排序"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn proxy_node_meta_uses_latest_non_negative_delay() {
    let response: ProxyGroups = serde_json::from_value(json!({
        "proxies": {
            "Proxy": {
                "name": "Proxy",
                "type": "Selector",
                "now": "HK",
                "all": ["HK"]
            },
            "HK": {
                "name": "HK",
                "type": "Vless",
                "all": null,
                "alive": true,
                "history": [
                    { "time": "2026-06-18T00:00:00Z", "delay": 120 },
                    { "time": "2026-06-18T00:01:00Z", "delay": -1 },
                    { "time": "2026-06-18T00:02:00Z", "delay": 88 }
                ]
            }
        }
    }))
    .expect("proxy response");

    let meta = super::proxy_node_meta_from_response(&response);
    assert_eq!(meta.len(), 2);
    let proxy = meta.get("Proxy").expect("Proxy meta");
    assert_eq!(proxy.proxy_type, "Selector");
    assert_eq!(proxy.delay_ms, None);
    assert_eq!(proxy.alive, None);
    let hk = meta.get("HK").expect("HK meta");
    assert_eq!(hk.proxy_type, "Vless");
    assert_eq!(hk.delay_ms, Some(88));
    assert_eq!(hk.alive, Some(true));
}

#[test]
fn proxy_selection_updates_cached_current_node() {
    let mut app = TuiApp {
        view: View::Proxies,
        proxy_pane: ProxyPane::Nodes,
        proxy_groups: vec![ProxyGroupRow {
            name: "GLOBAL".into(),
            now: "DIRECT".into(),
            nodes: vec!["DIRECT".into(), "香港节点".into(), "日本节点".into()],
            offline: false,
        }],
        ..TuiApp::default()
    };

    app.set_selected_proxy_group_now("GLOBAL", "日本节点");

    assert_eq!(app.proxy_groups[0].now, "日本节点");
    assert_eq!(app.proxy_node_index, 2);
}

#[test]
fn switch_status_message_explains_partial_apply_and_manual_action() {
    let status = actions::system::SwitchStatus {
        enabled: true,
        platform: "linux".into(),
        config_saved: true,
        runtime_generated: true,
        runtime_applied: Some(false),
        platform_applied: None,
        requires_core_restart: true,
        core_restarted: false,
        core_state: Some(KernelState::Stopped),
        runtime_path: Some("/tmp/runtime.yaml".into()),
        manual_action: Some("确认 mihomo 具备 CAP_NET_ADMIN".into()),
        message: "TUN 配置已保存，runtime 已重新生成；启动 Core 后生效".into(),
    };

    let message = switch_status_message("TUN", &status);

    assert!(message.contains("TUN已开启"));
    assert!(message.contains("配置已保存"));
    assert!(message.contains("runtime 已生成"));
    assert!(message.contains("启动或重启 Core 后生效"));
    assert!(message.contains("处理建议"));
    assert!(message.contains("CAP_NET_ADMIN"));
}

#[test]
fn proxy_group_summary_reports_empty_controller_states() {
    let empty: ProxyGroups = serde_json::from_value(json!({ "proxies": {} })).expect("empty groups");
    assert_eq!(proxy_group_load_summary(&empty), ProxyGroupLoadSummary::default());
    assert!(proxy_groups_empty_message(proxy_group_load_summary(&empty)).contains("未返回代理数据"));

    let leaf_only: ProxyGroups = serde_json::from_value(json!({
        "proxies": {
            "HK": { "name": "HK", "type": "Vless", "all": null }
        }
    }))
    .expect("leaf only");
    assert_eq!(
        proxy_group_load_summary(&leaf_only),
        ProxyGroupLoadSummary {
            entries: 1,
            groups: 0,
            nodes: 0
        }
    );
    assert!(proxy_groups_empty_message(proxy_group_load_summary(&leaf_only)).contains("没有可选策略组"));

    let ready: ProxyGroups = serde_json::from_value(json!({
        "proxies": {
            "Proxy": { "name": "Proxy", "type": "Selector", "all": ["HK", "SG"] },
            "HK": { "name": "HK", "type": "Vless", "all": null }
        }
    }))
    .expect("ready");
    assert_eq!(
        proxy_group_load_summary(&ready),
        ProxyGroupLoadSummary {
            entries: 2,
            groups: 1,
            nodes: 2
        }
    );
    assert!(proxy_group_load_summary(&ready).is_ready());
}

#[test]
fn runtime_proxy_summary_counts_generated_config_sections() {
    let summary = runtime_proxy_summary_from_yaml(
        r"proxies:
  - name: HK
    type: direct
proxy-providers:
  remote:
    type: http
    url: https://example.invalid/provider.yaml
    path: ./provider.yaml
proxy-groups:
  - name: Proxy
    type: select
    use:
      - remote
    proxies:
      - HK
rules:
  - MATCH,Proxy
",
    )
    .expect("runtime summary");

    assert_eq!(summary.proxies, 1);
    assert_eq!(summary.providers, 1);
    assert_eq!(summary.provider_names, vec!["remote"]);
    assert_eq!(summary.group_provider_names, vec!["remote"]);
    assert_eq!(summary.groups, 1);
    assert_eq!(summary.rules, 1);
    assert_eq!(summary.to_message(), "runtime：节点 1，Provider 1，策略组 1，规则 1");
    assert!(summary.uses_providers());
}

#[test]
fn runtime_proxy_groups_preview_reads_runtime_yaml() {
    let groups = runtime_proxy_groups_from_yaml(
        r"proxies:
  - name: HK
    type: direct
  - name: SG
    type: direct
proxy-providers:
  remote:
    type: http
    url: https://example.invalid/provider.yaml
    path: ./provider.yaml
proxy-groups:
  - name: Proxy
    type: select
    use:
      - remote
    proxies:
      - HK
      - DIRECT
  - name: Auto
    type: url-test
    include-all: true
rules:
  - MATCH,Proxy
",
    )
    .expect("runtime preview");

    assert_eq!(groups.len(), 3);
    assert!(groups.iter().all(|group| group.offline));
    assert_eq!(groups[0].name, "Proxy");
    assert_eq!(groups[0].now, "未预选");
    assert_eq!(
        groups[0].nodes,
        vec!["HK".to_owned(), "DIRECT".to_owned(), "Provider: remote".to_owned()]
    );
    assert!(groups[1].nodes.contains(&"SG".to_owned()));
    assert!(groups[1].nodes.contains(&"Provider: remote".to_owned()));
    assert_eq!(groups[2].name, "GLOBAL");
    assert_eq!(groups[2].nodes, vec!["HK".to_owned(), "SG".to_owned()]);
}

#[test]
fn provider_auto_refresh_prefers_controller_keys_filtered_by_runtime() {
    let runtime = runtime_proxy_summary_from_yaml(
        r"proxy-providers:
  remote:
    type: http
    url: https://example.invalid/provider.yaml
    path: ./provider.yaml
proxy-groups:
  - name: Proxy
    type: select
    use:
      - remote
rules:
  - MATCH,Proxy
",
    )
    .expect("runtime summary");
    let providers: ProxyProvidersResponse = serde_json::from_value(json!({
        "providers": {
            "other": {
                "name": "other",
                "vehicleType": "HTTP",
                "proxies": []
            },
            "remote": {
                "name": "Remote Display",
                "vehicleType": "HTTP",
                "proxies": [{ "name": "HK" }]
            }
        }
    }))
    .expect("providers");

    let names = provider_names_for_auto_refresh(Some(&runtime), Some(&providers));

    assert_eq!(names, vec!["remote"]);
}

#[test]
fn provider_auto_refresh_ignores_compatible_controller_providers() {
    let runtime = runtime_proxy_summary_from_yaml(
        r"proxy-providers:
  remote:
    type: http
    url: https://example.invalid/provider.yaml
    path: ./provider.yaml
proxy-groups:
  - name: Proxy
    type: select
    use:
      - remote
rules:
  - MATCH,Proxy
",
    )
    .expect("runtime summary");
    let providers: ProxyProvidersResponse = serde_json::from_value(json!({
        "providers": {
            "GLOBAL": {
                "name": "GLOBAL",
                "vehicleType": "Compatible",
                "proxies": [{ "name": "DIRECT" }]
            },
            "remote": {
                "name": "remote",
                "vehicleType": "HTTP",
                "proxies": [{ "name": "HK" }]
            }
        }
    }))
    .expect("providers");

    let names = provider_names_for_auto_refresh(Some(&runtime), Some(&providers));

    assert_eq!(names, vec!["remote"]);

    let compatible_only: ProxyProvidersResponse = serde_json::from_value(json!({
        "providers": {
            "GLOBAL": {
                "name": "GLOBAL",
                "vehicleType": "Compatible",
                "proxies": [{ "name": "DIRECT" }]
            }
        }
    }))
    .expect("compatible providers");

    let names = provider_names_for_auto_refresh(None, Some(&compatible_only));

    assert!(names.is_empty());
}

#[test]
fn provider_auto_refresh_prioritizes_group_use_refs_over_other_runtime_providers() {
    let runtime = runtime_proxy_summary_from_yaml(
        r"proxy-providers:
  unused-a:
    type: http
    url: https://example.invalid/a.yaml
    path: ./a.yaml
  used-b:
    type: http
    url: https://example.invalid/b.yaml
    path: ./b.yaml
proxy-groups:
  - name: Proxy
    type: select
    use:
      - used-b
rules:
  - MATCH,Proxy
",
    )
    .expect("runtime summary");
    let providers: ProxyProvidersResponse = serde_json::from_value(json!({
        "providers": {
            "unused-a": {
                "name": "unused-a",
                "vehicleType": "HTTP",
                "proxies": []
            },
            "used-b-controller-key": {
                "name": "used-b",
                "vehicleType": "HTTP",
                "proxies": [{ "name": "HK" }]
            }
        }
    }))
    .expect("providers");

    let names = provider_names_for_auto_refresh(Some(&runtime), Some(&providers));

    assert_eq!(names, vec!["used-b-controller-key", "unused-a"]);
}

#[test]
fn provider_auto_refresh_falls_back_to_runtime_names_before_controller_lists_providers() {
    let runtime = runtime_proxy_summary_from_yaml(
        r"proxy-providers:
  remote-a:
    type: http
    url: https://example.invalid/a.yaml
    path: ./a.yaml
  remote-b:
    type: http
    url: https://example.invalid/b.yaml
    path: ./b.yaml
proxy-groups:
  - name: Proxy
    type: select
    use:
      - remote-a
      - remote-b
rules:
  - MATCH,Proxy
",
    )
    .expect("runtime summary");

    let names = provider_names_for_auto_refresh(Some(&runtime), None);

    assert_eq!(names, vec!["remote-a", "remote-b"]);
}

#[test]
fn provider_auto_refresh_prioritizes_empty_controller_providers() {
    let providers: ProxyProvidersResponse = serde_json::from_value(json!({
        "providers": {
            "loaded": {
                "name": "loaded",
                "vehicleType": "HTTP",
                "proxies": [{ "name": "HK" }]
            },
            "empty": {
                "name": "empty",
                "vehicleType": "HTTP",
                "proxies": []
            }
        }
    }))
    .expect("providers");

    let names = provider_names_for_auto_refresh(None, Some(&providers));

    assert_eq!(names, vec!["empty", "loaded"]);
}
