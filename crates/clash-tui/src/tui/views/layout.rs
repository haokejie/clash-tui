use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    symbols::{self, border},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget as _, Widget as _,
    },
};

use crate::{terminal_display, tui::terminal_safe_text};

const ASCII_BORDER: border::Set = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

const ASCII_SCROLLBAR: symbols::scrollbar::Set = symbols::scrollbar::Set {
    track: "|",
    thumb: "#",
    begin: "^",
    end: "v",
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ThemeTokens {
    pub(crate) canvas: Color,
    pub(crate) text: Color,
    pub(crate) muted_text: Color,
    pub(crate) dim_text: Color,
    pub(crate) border: Color,
    pub(crate) title: Color,
    pub(crate) accent: Color,
    pub(crate) accent_text: Color,
    pub(crate) selection_bg: Color,
    pub(crate) selection_text: Color,
    pub(crate) success: Color,
    pub(crate) warning: Color,
    pub(crate) danger: Color,
    pub(crate) progress_empty: Color,
    pub(crate) scrollbar_track: Color,
    pub(crate) scrollbar_thumb: Color,
}

pub(crate) fn theme_tokens() -> ThemeTokens {
    theme_tokens_for(terminal_display::current_theme())
}

pub(crate) const fn theme_tokens_for(theme: terminal_display::TuiTheme) -> ThemeTokens {
    match theme {
        terminal_display::TuiTheme::Blue => ThemeTokens {
            canvas: Color::Rgb(31, 42, 58),
            text: Color::Rgb(216, 226, 239),
            muted_text: Color::Rgb(151, 163, 184),
            dim_text: Color::Rgb(117, 132, 154),
            border: Color::Rgb(54, 70, 95),
            title: Color::Rgb(116, 204, 244),
            accent: Color::Rgb(116, 204, 244),
            accent_text: Color::Rgb(4, 12, 25),
            selection_bg: Color::Rgb(116, 204, 244),
            selection_text: Color::Rgb(4, 12, 25),
            success: Color::Rgb(45, 212, 143),
            warning: Color::Rgb(250, 204, 21),
            danger: Color::Rgb(248, 113, 113),
            progress_empty: Color::Rgb(71, 86, 112),
            scrollbar_track: Color::Rgb(54, 70, 95),
            scrollbar_thumb: Color::Rgb(116, 204, 244),
        },
        terminal_display::TuiTheme::Orange => ThemeTokens {
            canvas: Color::Rgb(32, 24, 18),
            text: Color::Rgb(242, 231, 219),
            muted_text: Color::Rgb(188, 164, 139),
            dim_text: Color::Rgb(145, 118, 96),
            border: Color::Rgb(94, 66, 45),
            title: Color::Rgb(255, 162, 86),
            accent: Color::Rgb(244, 128, 55),
            accent_text: Color::Rgb(24, 13, 6),
            selection_bg: Color::Rgb(244, 128, 55),
            selection_text: Color::Rgb(24, 13, 6),
            success: Color::Rgb(70, 211, 145),
            warning: Color::Rgb(245, 183, 77),
            danger: Color::Rgb(248, 113, 113),
            progress_empty: Color::Rgb(82, 58, 41),
            scrollbar_track: Color::Rgb(94, 66, 45),
            scrollbar_thumb: Color::Rgb(244, 128, 55),
        },
    }
}

pub(crate) fn canvas_style() -> Style {
    let tokens = theme_tokens();
    Style::default().fg(tokens.text).bg(tokens.canvas)
}

pub(crate) fn paint_area(area: Rect, buffer: &mut Buffer) {
    buffer.set_style(area, canvas_style());
}

pub(crate) fn clear_area(area: Rect, buffer: &mut Buffer) {
    Clear.render(area, buffer);
    paint_area(area, buffer);
}

pub(crate) fn pad_right_display(value: &str, width: usize) -> String {
    let value = display_text(value);
    let value_width = display_width(&value);
    if value_width >= width {
        return fit_display_width(&value, width);
    }
    let mut padded = String::with_capacity(value.len() + width.saturating_sub(value_width));
    padded.push_str(&value);
    padded.push_str(&" ".repeat(width - value_width));
    padded
}

pub(crate) fn fit_display_width(value: &str, width: usize) -> String {
    let value = display_text(value);
    if display_width(&value) <= width {
        return value;
    }
    if width <= 3 {
        return take_display_width(&value, width);
    }
    let mut truncated = take_display_width(&value, width - 3);
    truncated.push_str("...");
    truncated
}

fn take_display_width(value: &str, width: usize) -> String {
    let mut output = String::new();
    for (index, ch) in value.char_indices() {
        let next = index + ch.len_utf8();
        if display_width(&value[..next]) > width {
            break;
        }
        output.push(ch);
    }
    output
}

pub(crate) fn display_width(value: &str) -> usize {
    Span::raw(display_text(value)).width()
}

pub(crate) fn table_text(value: &str, width: usize) -> String {
    fit_display_width(&stable_table_text(value), width)
}

pub(crate) fn table_cell_text(value: &str) -> String {
    display_text(&stable_table_text(value))
}

pub(crate) fn format_proxy_delay(delay: Option<i64>) -> String {
    match delay {
        None => "-".to_owned(),
        Some(-2) => "testing".to_owned(),
        Some(value) if value < 0 => "-".to_owned(),
        Some(0) => "Timeout".to_owned(),
        Some(value) if value > 100_000 => "Error".to_owned(),
        Some(value) if value >= 10_000 => "Timeout".to_owned(),
        Some(value) => format!("{value}ms"),
    }
}

pub(crate) fn proxy_delay_style(delay: Option<i64>) -> Style {
    match delay {
        Some(value) if value > 0 && value < 180 => ok_style(),
        Some(value) if value > 0 && value < 10_000 => warn_style(),
        Some(0) | Some(10_000..) => warn_style(),
        _ => muted_style(),
    }
}

pub(crate) fn themed_block(title: &'static str) -> Block<'static> {
    themed_block_with_title(title)
}

pub(crate) fn themed_block_with_title(title: impl AsRef<str>) -> Block<'static> {
    themed_block_with_title_for_mode(title, terminal_display::current_display_mode())
}

fn themed_block_with_title_for_mode(title: impl AsRef<str>, mode: terminal_display::TuiDisplayMode) -> Block<'static> {
    let mut block = Block::default()
        .title(Span::styled(display_text(title.as_ref()), title_style()))
        .borders(Borders::ALL)
        .border_style(border_style())
        .style(panel_style());
    if mode.uses_basic_symbols() {
        block = block.border_set(ASCII_BORDER);
    }
    block
}

pub(crate) fn render_vertical_scrollbar(
    area: Rect,
    buffer: &mut Buffer,
    content_len: usize,
    viewport_len: usize,
    position: usize,
) {
    if area.width == 0 || area.height < 2 || content_len <= viewport_len {
        return;
    }

    let mut state = ScrollbarState::new(content_len)
        .position(position)
        .viewport_content_length(viewport_len);
    let symbols = if terminal_display::current_display_mode().uses_basic_symbols() {
        ASCII_SCROLLBAR
    } else {
        symbols::scrollbar::VERTICAL
    };
    Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .symbols(symbols)
        .begin_symbol(None)
        .end_symbol(None)
        .track_style(scrollbar_track_style())
        .thumb_style(scrollbar_thumb_style())
        .render(area, buffer, &mut state);
}

pub(crate) fn panel_style() -> Style {
    canvas_style()
}

pub(crate) fn border_style() -> Style {
    let tokens = theme_tokens();
    Style::default().fg(tokens.border).bg(tokens.canvas)
}

pub(crate) fn title_style() -> Style {
    let tokens = theme_tokens();
    Style::default()
        .fg(tokens.title)
        .bg(tokens.canvas)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn accent_style() -> Style {
    let tokens = theme_tokens();
    Style::default()
        .fg(tokens.accent)
        .bg(tokens.canvas)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn selected_style() -> Style {
    let tokens = theme_tokens();
    Style::default()
        .fg(tokens.selection_text)
        .bg(tokens.selection_bg)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn muted_style() -> Style {
    let tokens = theme_tokens();
    Style::default().fg(tokens.muted_text).bg(tokens.canvas)
}

pub(crate) fn progress_empty_style() -> Style {
    let tokens = theme_tokens();
    Style::default().fg(tokens.progress_empty).bg(tokens.canvas)
}

pub(crate) fn scrollbar_track_style() -> Style {
    let tokens = theme_tokens();
    Style::default().fg(tokens.scrollbar_track).bg(tokens.canvas)
}

pub(crate) fn scrollbar_thumb_style() -> Style {
    let tokens = theme_tokens();
    Style::default()
        .fg(tokens.scrollbar_thumb)
        .bg(tokens.canvas)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn dim_style() -> Style {
    let tokens = theme_tokens();
    Style::default().fg(tokens.dim_text).bg(tokens.canvas)
}

pub(crate) fn ok_style() -> Style {
    let tokens = theme_tokens();
    Style::default()
        .fg(tokens.success)
        .bg(tokens.canvas)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn warn_style() -> Style {
    let tokens = theme_tokens();
    Style::default()
        .fg(tokens.warning)
        .bg(tokens.canvas)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn danger_style() -> Style {
    let tokens = theme_tokens();
    Style::default()
        .fg(tokens.danger)
        .bg(tokens.canvas)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn stable_table_text(value: &str) -> String {
    let sanitized = terminal_safe_text(value);
    let without_icons = sanitized
        .chars()
        .filter(|ch| !is_unstable_terminal_icon(*ch))
        .collect::<String>();
    let compact = without_icons.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() { "-".to_owned() } else { compact }
}

pub(crate) fn display_text(value: &str) -> String {
    terminal_display::normalize_text_for_mode(value)
}

pub(crate) fn display_line(value: impl AsRef<str>) -> Line<'static> {
    Line::from(display_text(value.as_ref()))
}

pub(crate) fn horizontal_symbol() -> &'static str {
    if terminal_display::current_display_mode().uses_basic_symbols() {
        "-"
    } else {
        "─"
    }
}

pub(crate) fn vertical_symbol() -> &'static str {
    if terminal_display::current_display_mode().uses_basic_symbols() {
        "|"
    } else {
        "│"
    }
}

pub(crate) fn cross_symbol() -> &'static str {
    if terminal_display::current_display_mode().uses_basic_symbols() {
        "+"
    } else {
        "┼"
    }
}

pub(crate) fn tee_left_symbol() -> &'static str {
    if terminal_display::current_display_mode().uses_basic_symbols() {
        "+"
    } else {
        "├"
    }
}

pub(crate) fn tee_right_symbol() -> &'static str {
    if terminal_display::current_display_mode().uses_basic_symbols() {
        "+"
    } else {
        "┤"
    }
}

pub(crate) fn tee_top_symbol() -> &'static str {
    if terminal_display::current_display_mode().uses_basic_symbols() {
        "+"
    } else {
        "┬"
    }
}

pub(crate) fn tee_bottom_symbol() -> &'static str {
    if terminal_display::current_display_mode().uses_basic_symbols() {
        "+"
    } else {
        "┴"
    }
}

pub(crate) fn progress_symbols() -> (&'static str, &'static str) {
    if terminal_display::current_display_mode().uses_basic_symbols() {
        ("#", ".")
    } else {
        ("█", "░")
    }
}

const fn is_unstable_terminal_icon(ch: char) -> bool {
    let code = ch as u32;
    matches!(
        code,
        0x200D | 0xFE00..=0xFE0F | 0x2600..=0x27BF | 0x1F1E6..=0x1F1FF | 0x1F300..=0x1FAFF
    )
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend, style::Color, widgets::Widget as _};

    use crate::terminal_display::{TuiDisplayMode, TuiTheme};

    #[test]
    fn blue_theme_uses_original_tab_background_as_global_canvas() {
        let tokens = super::theme_tokens_for(TuiTheme::Blue);
        assert_eq!(tokens.canvas, Color::Rgb(31, 42, 58));
        assert_eq!(tokens.title, Color::Rgb(116, 204, 244));
        assert_eq!(tokens.selection_bg, tokens.accent);
    }

    #[test]
    fn orange_theme_keeps_dark_canvas_with_orange_accent() {
        let tokens = super::theme_tokens_for(TuiTheme::Orange);
        assert_eq!(tokens.canvas, Color::Rgb(32, 24, 18));
        assert_eq!(tokens.title, Color::Rgb(255, 162, 86));
        assert_eq!(tokens.selection_bg, tokens.accent);
    }

    #[test]
    fn themed_block_dynamic_title_honors_basic_display_symbols() {
        let backend = TestBackend::new(28, 5);
        let mut terminal = Terminal::new(backend).expect("terminal");

        terminal
            .draw(|frame| {
                super::themed_block_with_title_for_mode("动态标题", TuiDisplayMode::Basic)
                    .render(frame.area(), frame.buffer_mut());
            })
            .expect("draw");

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((0, 0)).expect("top left").symbol(), "+");
        assert_eq!(buffer.cell((27, 0)).expect("top right").symbol(), "+");
        assert_eq!(buffer.cell((0, 4)).expect("bottom left").symbol(), "+");
        assert_eq!(buffer.cell((27, 4)).expect("bottom right").symbol(), "+");
        assert_eq!(buffer.cell((0, 1)).expect("left border").symbol(), "|");
        assert_eq!(buffer.cell((12, 0)).expect("top border").symbol(), "-");
    }
}
