use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{Paragraph, Tabs, Widget as _, Wrap},
};

use crate::{state::AppState, tui::views};

use super::{
    models::{
        ConfirmAction, DashboardProxyPopup, InputState, InputTarget, MIN_TUI_HEIGHT, MIN_TUI_WIDTH, SettingRow, View,
    },
    state::TuiApp,
    text::{status_history_text, terminal_safe_text},
};

pub fn render(area: Rect, buffer: &mut ratatui::buffer::Buffer, app: &TuiApp, state: &AppState) {
    views::layout::paint_area(area, buffer);
    if area.width < MIN_TUI_WIDTH || area.height < MIN_TUI_HEIGHT {
        render_terminal_too_small(area, buffer);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(3), Constraint::Length(3)])
        .split(area);
    render_tabs(chunks[0], buffer, app.view);

    if app.show_help {
        render_help(chunks[1], buffer, app.view);
    } else {
        match app.view {
            View::Dashboard => views::dashboard::render(chunks[1], buffer, app),
            View::Profiles => views::profiles::render(chunks[1], buffer, app),
            View::Proxies => views::proxies::render(chunks[1], buffer, state, app),
            View::Logs => views::logs::render(chunks[1], buffer, app),
            View::Settings => views::settings::render(chunks[1], buffer, app),
            View::Rules => views::rules::render(chunks[1], buffer, app),
            View::Connections => views::connections::render(chunks[1], buffer, app),
            View::Jobs => views::jobs::render(chunks[1], buffer, app),
        }
    }

    Paragraph::new(footer_lines(app, usize::from(chunks[2].width)))
        .style(views::layout::panel_style())
        .render(chunks[2], buffer);

    render_transient_modal(area, buffer, app);
}

const PUNCTUATION_TEST_PAGE_SIZE: usize = 8;

pub(crate) fn punctuation_test_page_count() -> usize {
    punctuation_test_samples()
        .len()
        .div_ceil(PUNCTUATION_TEST_PAGE_SIZE)
        .max(1)
}

pub(crate) fn render_punctuation_test_page(area: Rect, buffer: &mut ratatui::buffer::Buffer, page: usize) {
    views::layout::paint_area(area, buffer);
    if area.width < 120 || area.height < 34 {
        Paragraph::new(vec![
            views::layout::display_line(format!("当前终端：{}x{}", area.width, area.height)),
            views::layout::display_line("中文标点测试页建议至少 120x34。"),
            views::layout::display_line("按 q 或 Esc 退出。"),
        ])
        .block(views::layout::themed_block("中文标点测试"))
        .render(area, buffer);
        return;
    }

    let outer = views::layout::themed_block("中文标点测试");
    let inner = outer.inner(area);
    outer.render(area, buffer);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Min(8), Constraint::Length(2)])
        .split(inner);
    let page_count = punctuation_test_page_count();
    let page = page.min(page_count.saturating_sub(1));
    Paragraph::new(vec![
        views::layout::display_line("垂直三段独立测试：原始、常见、优化。"),
        views::layout::display_line("观察每行末尾的 | 是否贴近右边框且不越界。"),
        views::layout::display_line(format!(
            "第 {}/{} 页；n 下一页，p 上一页，q/Esc 退出。",
            page + 1,
            page_count
        )),
    ])
    .render(chunks[0], buffer);

    render_punctuation_sample_sections(chunks[1], buffer, page);
    Paragraph::new(views::layout::display_line(
        "启动方式：CLASH_TUI_PUNCTUATION_TEST=1 clash-tui",
    ))
    .style(views::layout::dim_style())
    .render(chunks[2], buffer);
}

fn render_punctuation_sample_sections(area: Rect, buffer: &mut ratatui::buffer::Buffer, page: usize) {
    let start = page.saturating_mul(PUNCTUATION_TEST_PAGE_SIZE);
    let end = (start + PUNCTUATION_TEST_PAGE_SIZE).min(punctuation_test_samples().len());
    let samples = &punctuation_test_samples()[start..end];
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(area);

    render_punctuation_mode_block(chunks[0], buffer, "原始中文标点", samples, None);
    render_punctuation_mode_block(
        chunks[1],
        buffer,
        "常见标点",
        samples,
        Some(crate::terminal_display::TuiPunctuationMode::Common),
    );
    render_punctuation_mode_block(
        chunks[2],
        buffer,
        "优化标点",
        samples,
        Some(crate::terminal_display::TuiPunctuationMode::ColonComma),
    );
}

fn render_punctuation_mode_block(
    area: Rect,
    buffer: &mut ratatui::buffer::Buffer,
    title: &'static str,
    samples: &[PunctuationTestSample],
    mode: Option<crate::terminal_display::TuiPunctuationMode>,
) {
    let inner_width = usize::from(area.width.saturating_sub(2)).max(1);
    let max_lines = usize::from(area.height.saturating_sub(2));
    let lines = samples
        .iter()
        .take(max_lines)
        .map(|sample| {
            let value = mode
                .map(|mode| crate::terminal_display::normalize_text_for_punctuation_mode(sample.value, mode))
                .unwrap_or_else(|| sample.value.to_owned());
            punctuation_probe_line(sample.title, &value, inner_width)
        })
        .collect::<Vec<_>>();
    Paragraph::new(lines)
        .block(views::layout::themed_block(title))
        .render(area, buffer);
}

fn punctuation_probe_line(label: &str, value: &str, width: usize) -> Line<'static> {
    let mut line = format!("{label} {value}");
    let marker_width = 1;
    let current_width = Span::raw(line.as_str()).width();
    if width > current_width + marker_width {
        line.push_str(&" ".repeat(width - current_width - marker_width));
    }
    line.push('|');
    Line::from(line)
}

struct PunctuationTestSample {
    title: &'static str,
    value: &'static str,
}

const fn punctuation_test_samples() -> &'static [PunctuationTestSample] {
    &[
        PunctuationTestSample {
            title: "冒号",
            value: "A：B",
        },
        PunctuationTestSample {
            title: "逗号",
            value: "A，B",
        },
        PunctuationTestSample {
            title: "分号",
            value: "A；B",
        },
        PunctuationTestSample {
            title: "句号",
            value: "A。B",
        },
        PunctuationTestSample {
            title: "问号",
            value: "A？B",
        },
        PunctuationTestSample {
            title: "感叹",
            value: "A！B",
        },
        PunctuationTestSample {
            title: "顿号",
            value: "A、B",
        },
        PunctuationTestSample {
            title: "括号",
            value: "A（B）C",
        },
        PunctuationTestSample {
            title: "方括",
            value: "A【B】C",
        },
        PunctuationTestSample {
            title: "引号",
            value: "A“B”C",
        },
        PunctuationTestSample {
            title: "单引",
            value: "A‘B’C",
        },
        PunctuationTestSample {
            title: "书名",
            value: "A《B》C",
        },
        PunctuationTestSample {
            title: "线条",
            value: "A｜B／C＼D",
        },
        PunctuationTestSample {
            title: "破折",
            value: "A—B–C－D",
        },
        PunctuationTestSample {
            title: "省略",
            value: "A…B",
        },
        PunctuationTestSample {
            title: "空格",
            value: "A　B",
        },
    ]
}

fn render_terminal_too_small(area: Rect, buffer: &mut ratatui::buffer::Buffer) {
    views::layout::paint_area(area, buffer);
    let lines = vec![
        views::layout::display_line(format!("当前终端：{}x{}", area.width, area.height)),
        views::layout::display_line(format!("建议至少：{}x{}", MIN_TUI_WIDTH, MIN_TUI_HEIGHT)),
        views::layout::display_line("请放大终端窗口后继续，SSH 会话需要设置有效 rows/cols。"),
        views::layout::display_line("按 q 退出。"),
    ];
    Paragraph::new(lines)
        .block(views::layout::themed_block("终端尺寸不足"))
        .render(area, buffer);
}

pub(crate) fn render_transient_modal(area: Rect, buffer: &mut ratatui::buffer::Buffer, app: &TuiApp) {
    if app.show_help {
        return;
    }
    if let Some(busy) = &app.busy {
        let modal = modal_rect(area, 74, 7);
        views::layout::clear_area(modal, buffer);
        Paragraph::new(vec![
            Line::from(modal_line_text(&busy.message, modal)),
            Line::from(""),
            views::layout::display_line("正在处理，请稍候。完成后会自动刷新结果。"),
        ])
        .block(views::layout::themed_block("处理中"))
        .wrap(Wrap { trim: true })
        .render(modal, buffer);
        return;
    }
    if let Some(confirm) = &app.confirm {
        let modal = modal_rect(area, 74, 7);
        views::layout::clear_area(modal, buffer);
        Paragraph::new(vec![
            Line::from(modal_line_text(&confirm.prompt, modal)),
            Line::from(""),
            views::layout::display_line("按 y 确认，按 n 或 Esc 取消。"),
        ])
        .block(views::layout::themed_block("确认操作"))
        .wrap(Wrap { trim: true })
        .render(modal, buffer);
        return;
    }
    if let Some(input) = &app.input {
        let modal = modal_rect(area, 74, 8);
        views::layout::clear_area(modal, buffer);
        Paragraph::new(vec![
            views::layout::display_line(input_target_title(input.target)),
            Line::from(modal_line_text(&input_display_value(input), modal)),
            Line::from(""),
            views::layout::display_line("输入或粘贴文本，Enter 应用，Backspace 删除，Esc 取消。"),
        ])
        .block(views::layout::themed_block("输入"))
        .wrap(Wrap { trim: true })
        .render(modal, buffer);
        return;
    }
    if let Some(detail) = &app.detail {
        let detail_height = area.height.saturating_sub(4).clamp(18, 26);
        let modal = modal_rect(area, 82, detail_height);
        let max_detail_lines = usize::from(modal.height.saturating_sub(4)).max(1);
        views::layout::clear_area(modal, buffer);
        let mut lines = detail
            .lines
            .iter()
            .take(max_detail_lines)
            .map(|line| Line::from(modal_line_text(line, modal)))
            .collect::<Vec<_>>();
        if detail.lines.len() > max_detail_lines {
            lines.push(Line::from("..."));
        }
        lines.push(Line::from(""));
        lines.push(views::layout::display_line("按 Esc/Enter 关闭详情，按 q 退出。"));
        Paragraph::new(lines)
            .block(views::layout::themed_block_with_title(&detail.title))
            .wrap(Wrap { trim: true })
            .render(modal, buffer);
        return;
    }
    if app.provider_dialog.is_some() {
        views::provider_dialog::render(area, buffer, app);
        return;
    }
    if status_is_error(&app.status) {
        let modal = modal_rect(area, 74, 7);
        views::layout::clear_area(modal, buffer);
        Paragraph::new(vec![
            Line::from(Span::styled(
                modal_line_text(&app.status, modal),
                views::layout::danger_style(),
            )),
            Line::from(""),
            views::layout::display_line("可以继续切页、刷新或按 ? 查看帮助。"),
        ])
        .block(views::layout::themed_block("错误提示"))
        .wrap(Wrap { trim: true })
        .render(modal, buffer);
    }
}

pub(crate) fn footer_line_strings(app: &TuiApp, width: usize) -> [String; 3] {
    let interaction = app.interaction_line();
    [
        terminal_status_line(&app.status, width),
        terminal_status_line(&interaction, width),
        help_status_line(app, width),
    ]
}

pub(crate) fn footer_lines(app: &TuiApp, width: usize) -> Vec<Line<'static>> {
    footer_line_strings(app, width).into_iter().map(Line::from).collect()
}

fn terminal_status_line(value: &str, width: usize) -> String {
    views::layout::fit_display_width(&status_history_text(value), width)
}

fn help_status_line(app: &TuiApp, width: usize) -> String {
    if app.show_help {
        return terminal_status_line("键位：Esc/? 关闭帮助  q 退出", width);
    }
    if app.confirm.is_some() {
        return terminal_status_line("键位：y 确认  n/Esc 取消", width);
    }
    if app.input.is_some() {
        return terminal_status_line("键位：输入/粘贴文本  Backspace 删除  Enter 应用  Esc 取消", width);
    }
    if app.provider_dialog.is_some() {
        return terminal_status_line("键位：↑↓选择  Enter/u 更新  a 全部更新  r 刷新  Esc 关闭", width);
    }

    compact_help_line(app, width)
}

fn modal_line_text(value: &str, area: Rect) -> String {
    let width = usize::from(area.width.saturating_sub(4)).max(1);
    terminal_status_line(value, width)
}

fn modal_rect(area: Rect, width_percent: u16, height: u16) -> Rect {
    let width = area
        .width
        .saturating_mul(width_percent)
        .saturating_div(100)
        .clamp(40, area.width.saturating_sub(4).max(1));
    let height = height.min(area.height.saturating_sub(2).max(1));
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

pub(crate) const fn input_target_title(target: InputTarget) -> &'static str {
    match target {
        InputTarget::Search(_) => "过滤条件",
        InputTarget::MixedPort => "混合端口",
        InputTarget::ExternalControllerPort => "外部控制端口",
        InputTarget::ImportLocalProfilePath => "本地配置路径",
        InputTarget::ImportSubscriptionUrl => "订阅链接",
    }
}

pub(crate) fn input_display_value(input: &InputState) -> String {
    if input.value.is_empty() {
        return "当前输入：空".into();
    }
    match input.target {
        InputTarget::ImportSubscriptionUrl => {
            format!("当前输入：[订阅链接已输入，{} 个字符]", input.value.chars().count())
        }
        _ => format!("当前输入：{}", terminal_safe_text(&input.value)),
    }
}

pub(crate) const fn input_target_busy_message(target: InputTarget) -> Option<&'static str> {
    match target {
        InputTarget::Search(_) => None,
        InputTarget::MixedPort => Some("正在保存混合端口设置..."),
        InputTarget::ExternalControllerPort => Some("正在保存外部控制端口..."),
        InputTarget::ImportLocalProfilePath => Some("正在导入本地配置..."),
        InputTarget::ImportSubscriptionUrl => Some("正在导入订阅并等待代理组加载..."),
    }
}

pub(crate) const fn confirm_action_busy_message(action: &ConfirmAction) -> &'static str {
    match action {
        ConfirmAction::SwitchProfile { .. } => "正在切换订阅并刷新 runtime...",
        ConfirmAction::DeleteProfile { .. } => "正在删除订阅...",
        ConfirmAction::CloseConnection { .. } => "正在关闭连接...",
        ConfirmAction::CloseAllConnections => "正在关闭全部连接...",
        ConfirmAction::ClearLogs => "正在清空日志...",
        ConfirmAction::ToggleTun { .. } => "正在切换 TUN 并刷新 runtime...",
        ConfirmAction::ToggleSystemProxy { .. } => "正在切换系统代理...",
        ConfirmAction::ToggleExternalController { .. } => "正在切换外部控制器并刷新 runtime...",
        ConfirmAction::SetExternalControllerPort { .. } => "正在修改外部控制端口并刷新 runtime...",
        ConfirmAction::ToggleCoreLog { .. } => "正在切换核心日志并重启 Core...",
    }
}

pub(crate) const fn settings_row_busy_message(row: SettingRow) -> Option<&'static str> {
    match row {
        SettingRow::Dns
        | SettingRow::Ipv6
        | SettingRow::AllowLan
        | SettingRow::UnifiedDelay
        | SettingRow::TuiTheme
        | SettingRow::TuiDisplayMode
        | SettingRow::TuiPunctuationMode
        | SettingRow::LogLevel => Some("正在应用设置..."),
        SettingRow::MixedPort
        | SettingRow::CoreLog
        | SettingRow::ExternalController
        | SettingRow::ExternalControllerPort
        | SettingRow::Tun
        | SettingRow::SystemProxy => None,
    }
}

fn status_is_error(status: &str) -> bool {
    status.starts_with("错误：")
        || status.contains("失败：")
        || status.contains("失败；")
        || status.contains("失败，")
        || status.ends_with("失败")
}

fn render_help(area: Rect, buffer: &mut ratatui::buffer::Buffer, current: View) {
    let lines = vec![
        views::layout::display_line("当前：完整键位帮助。Esc 或 ? 关闭帮助，q 退出程序。"),
        Line::from(""),
        views::layout::display_line(
            "全局：1-8 直达页面 | Tab/←→/h/l 切页 | s 启停核心 | r 刷新 | D 诊断 | E 导出诊断快照 | n 消息历史 | ↑↓/j/k 选择 | Home/End 到首尾 | / 过滤",
        ),
        views::layout::display_line("粘贴：任意页面直接粘贴 http(s) 订阅链接，确认后导入并激活。"),
        views::layout::display_line(
            "总览：s 启停核心 | Enter 展开/应用节点 | g 展开代理组 | P 系统代理 | T TUN | d DNS | R 重启核心 | m 模式",
        ),
        views::layout::display_line(
            "订阅：Enter 切换 | i 手动输入订阅链接 | o 导入本地配置文件 | u 更新选中 | a 更新全部 | d/Delete 删除",
        ),
        views::layout::display_line(
            "代理：左侧代理组、右侧节点 | f 切焦点 | Enter 在组上定位节点、在节点上应用 | S 排序 | p Provider | m 切模式 | / 过滤",
        ),
        views::layout::display_line("日志：↑↓ 查看历史 | f 开关跟随 | L 循环等级 | / 按日志内容过滤 | x 清屏"),
        views::layout::display_line("设置：Enter 切换/循环选中项 | e 编辑混合端口 | TUN/系统代理会二次确认"),
        views::layout::display_line("规则：↑↓ 浏览 | p Provider | / 按类型、内容、代理过滤"),
        views::layout::display_line("连接：Enter 查看详情 | d/Delete 关闭选中连接 | c 关闭全部连接"),
        views::layout::display_line("任务：Enter 查看详情 | R 重试订阅更新任务 | c 取消运行中任务"),
        views::layout::display_line("输入模式：直接粘贴或输入文本，Enter 应用，Backspace 删除，Esc 取消。"),
        views::layout::display_line("确认模式：y 确认，n 或 Esc 取消。"),
    ];
    Paragraph::new(lines)
        .block(views::layout::themed_block_with_title(format!(
            "帮助 - {}",
            current.title()
        )))
        .render(area, buffer);
}

fn compact_help_line(app: &TuiApp, width: usize) -> String {
    let required = ["1-8/Tab 切页", "r 刷新", "? 帮助"];
    let preferred = match app.view {
        View::Dashboard => match app.dashboard_proxy_popup {
            DashboardProxyPopup::Groups => vec!["↑↓选组", "Enter 定位节点", "Esc 收起", "3 代理页"],
            DashboardProxyPopup::Nodes => vec!["↑↓选节点", "Enter 应用", "Esc 收起", "g 代理组", "3 代理页"],
            DashboardProxyPopup::None => vec!["Enter 节点", "g 代理组", "P 系统代理", "T TUN", "d DNS", "m 模式"],
        },
        View::Profiles => vec![
            "↑↓选择",
            "Enter 切换",
            "i 输入URL",
            "o 本地导入",
            "u 更新",
            "a 全部更新",
            "d 删除",
            "/ 过滤",
        ],
        View::Proxies => {
            let mut segments = vec!["f 切焦点", "Enter 应用", "S 排序", "m 模式", "/ 过滤"];
            if !app.proxy_providers.is_empty() {
                segments.insert(3, "p Provider");
            }
            segments
        }
        View::Logs => vec!["↑↓滚动", "f 跟随", "L 等级", "/ 过滤", "x 清屏"],
        View::Settings => vec!["↑↓选择", "Enter 切换", "e 编辑"],
        View::Rules => {
            let mut segments = vec!["↑↓查看", "/ 过滤"];
            if !app.rule_providers.is_empty() {
                segments.insert(1, "p Provider");
            }
            segments
        }
        View::Connections => vec!["↑↓选择", "Enter 详情", "d 关闭", "c 全部关闭", "/ 过滤"],
        View::Jobs => vec!["↑↓选择", "Enter 详情", "R 重试", "c 取消", "/ 过滤"],
    };
    let secondary = ["s Core", "D 诊断", "E 快照", "n 消息", "q 退出"];
    compact_key_segments(&required, &preferred, &secondary, width)
}

fn compact_key_segments(required: &[&str], preferred: &[&str], secondary: &[&str], width: usize) -> String {
    let mut segments = required.to_vec();
    let minimum_segments = segments.len();
    let mut hidden = false;

    for segment in preferred.iter().chain(secondary.iter()).copied() {
        let mut candidate = segments.clone();
        candidate.push(segment);
        if views::layout::display_width(&key_segments_line(&candidate, false)) <= width {
            segments.push(segment);
        } else {
            hidden = true;
        }
    }

    if hidden {
        while segments.len() > minimum_segments
            && views::layout::display_width(&key_segments_line(&segments, true)) > width
        {
            segments.pop();
        }
        let line = key_segments_line(&segments, true);
        if views::layout::display_width(&line) <= width {
            return line;
        }
    }

    let line = key_segments_line(&segments, false);
    if views::layout::display_width(&line) <= width {
        line
    } else {
        views::layout::fit_display_width(&line, width)
    }
}

fn key_segments_line(segments: &[&str], more: bool) -> String {
    let mut parts = segments.to_vec();
    if more {
        parts.push("…更多");
    }
    format!("键位：{}", parts.join("  "))
}

fn render_tabs(area: Rect, buffer: &mut ratatui::buffer::Buffer, current: View) {
    let titles = View::ALL
        .iter()
        .enumerate()
        .map(|(index, view)| Line::from(Span::raw(format!("{} {}", index + 1, view.title()))))
        .collect::<Vec<_>>();
    let index = View::ALL.iter().position(|view| *view == current).unwrap_or(0);
    Tabs::new(titles)
        .select(index)
        .block(views::layout::themed_block("clash-tui"))
        .style(views::layout::panel_style())
        .highlight_style(views::layout::selected_style().add_modifier(Modifier::BOLD))
        .render(area, buffer);
}
