use std::{
    env,
    sync::atomic::{AtomicU8, Ordering},
};

use clash_core::IAppSettings;
use serde::Serialize;

pub const TUI_DISPLAY_MODE_ENV: &str = "CLASH_TUI_DISPLAY_MODE";
pub const TUI_PUNCTUATION_MODE_ENV: &str = "CLASH_TUI_PUNCTUATION_MODE";
pub const TUI_THEME_ENV: &str = "CLASH_TUI_THEME";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TuiDisplayMode {
    Standard,
    Basic,
}

impl TuiDisplayMode {
    pub const fn config_value(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Basic => "basic",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Standard => "标准",
            Self::Basic => "基础线框",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Standard => Self::Basic,
            Self::Basic => Self::Standard,
        }
    }

    pub const fn uses_basic_symbols(self) -> bool {
        matches!(self, Self::Basic)
    }

    const fn from_index(value: u8) -> Self {
        match value {
            1 => Self::Basic,
            _ => Self::Standard,
        }
    }

    const fn index(self) -> u8 {
        match self {
            Self::Standard => 0,
            Self::Basic => 1,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TuiModeSummary {
    pub configured: String,
    pub configured_label: String,
    pub effective: String,
    pub effective_label: String,
    pub overridden: bool,
    pub override_value: Option<String>,
    pub env_var: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TuiPunctuationMode {
    Preserve,
    ColonComma,
    Common,
}

impl TuiPunctuationMode {
    pub const fn config_value(self) -> &'static str {
        match self {
            Self::Preserve => "preserve",
            Self::ColonComma => "colon-comma",
            Self::Common => "common",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Preserve => "保留",
            Self::ColonComma => "优化标点",
            Self::Common => "常见标点",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Preserve => Self::ColonComma,
            Self::ColonComma => Self::Common,
            Self::Common => Self::Preserve,
        }
    }

    const fn from_index(value: u8) -> Self {
        match value {
            1 => Self::ColonComma,
            2 => Self::Common,
            _ => Self::Preserve,
        }
    }

    const fn index(self) -> u8 {
        match self {
            Self::Preserve => 0,
            Self::ColonComma => 1,
            Self::Common => 2,
        }
    }
}

pub type TuiDisplayModeSummary = TuiModeSummary;
pub type TuiPunctuationModeSummary = TuiModeSummary;
pub type TuiThemeSummary = TuiModeSummary;

static CURRENT_DISPLAY_MODE: AtomicU8 = AtomicU8::new(0);
static CURRENT_PUNCTUATION_MODE: AtomicU8 = AtomicU8::new(0);
static CURRENT_THEME: AtomicU8 = AtomicU8::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TuiTheme {
    Blue,
    Orange,
}

impl TuiTheme {
    pub const fn config_value(self) -> &'static str {
        match self {
            Self::Blue => "blue",
            Self::Orange => "orange",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Blue => "蓝色",
            Self::Orange => "深橙",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Blue => Self::Orange,
            Self::Orange => Self::Blue,
        }
    }

    const fn from_index(value: u8) -> Self {
        match value {
            1 => Self::Orange,
            _ => Self::Blue,
        }
    }

    const fn index(self) -> u8 {
        match self {
            Self::Blue => 0,
            Self::Orange => 1,
        }
    }
}

pub fn configured_mode(app_settings: &IAppSettings) -> TuiDisplayMode {
    parse_display_mode(app_settings.tui_display_mode.as_deref()).unwrap_or(TuiDisplayMode::Standard)
}

pub fn summary(app_settings: &IAppSettings) -> TuiDisplayModeSummary {
    mode_summary(
        configured_mode(app_settings),
        TUI_DISPLAY_MODE_ENV,
        parse_display_mode,
        TuiDisplayMode::config_value,
        TuiDisplayMode::label,
    )
}

pub fn configured_punctuation_mode(app_settings: &IAppSettings) -> TuiPunctuationMode {
    parse_punctuation_mode(app_settings.tui_punctuation_mode.as_deref()).unwrap_or(TuiPunctuationMode::Preserve)
}

pub fn punctuation_summary(app_settings: &IAppSettings) -> TuiPunctuationModeSummary {
    mode_summary(
        configured_punctuation_mode(app_settings),
        TUI_PUNCTUATION_MODE_ENV,
        parse_punctuation_mode,
        TuiPunctuationMode::config_value,
        TuiPunctuationMode::label,
    )
}

pub fn configured_theme(app_settings: &IAppSettings) -> TuiTheme {
    parse_theme(app_settings.tui_theme.as_deref()).unwrap_or(TuiTheme::Orange)
}

pub fn theme_summary(app_settings: &IAppSettings) -> TuiThemeSummary {
    mode_summary(
        configured_theme(app_settings),
        TUI_THEME_ENV,
        parse_theme,
        TuiTheme::config_value,
        TuiTheme::label,
    )
}

fn mode_summary<T>(
    configured: T,
    env_var: &'static str,
    parse: fn(Option<&str>) -> Option<T>,
    config_value: fn(T) -> &'static str,
    label: fn(T) -> &'static str,
) -> TuiModeSummary
where
    T: Copy,
{
    let override_value = env::var(env_var).ok().filter(|value| !value.trim().is_empty());
    let override_mode = override_value.as_deref().and_then(|value| parse(Some(value)));
    let effective = override_mode.unwrap_or(configured);
    TuiModeSummary {
        configured: config_value(configured).to_owned(),
        configured_label: label(configured).to_owned(),
        effective: config_value(effective).to_owned(),
        effective_label: label(effective).to_owned(),
        overridden: override_mode.is_some(),
        override_value: override_mode.and(override_value),
        env_var,
    }
}

pub fn mode_from_summary(summary: &TuiDisplayModeSummary) -> TuiDisplayMode {
    parse_display_mode(Some(&summary.effective)).unwrap_or(TuiDisplayMode::Standard)
}

pub fn punctuation_mode_from_summary(summary: &TuiPunctuationModeSummary) -> TuiPunctuationMode {
    parse_punctuation_mode(Some(&summary.effective)).unwrap_or(TuiPunctuationMode::Preserve)
}

pub fn theme_from_summary(summary: &TuiThemeSummary) -> TuiTheme {
    parse_theme(Some(&summary.effective)).unwrap_or(TuiTheme::Orange)
}

pub fn current_display_mode() -> TuiDisplayMode {
    TuiDisplayMode::from_index(CURRENT_DISPLAY_MODE.load(Ordering::Relaxed))
}

pub fn set_current_display_mode(mode: TuiDisplayMode) {
    CURRENT_DISPLAY_MODE.store(mode.index(), Ordering::Relaxed);
}

pub fn current_punctuation_mode() -> TuiPunctuationMode {
    TuiPunctuationMode::from_index(CURRENT_PUNCTUATION_MODE.load(Ordering::Relaxed))
}

pub fn set_current_punctuation_mode(mode: TuiPunctuationMode) {
    CURRENT_PUNCTUATION_MODE.store(mode.index(), Ordering::Relaxed);
}

pub fn current_theme() -> TuiTheme {
    TuiTheme::from_index(CURRENT_THEME.load(Ordering::Relaxed))
}

pub fn set_current_theme(theme: TuiTheme) {
    CURRENT_THEME.store(theme.index(), Ordering::Relaxed);
}

pub fn parse_display_mode(value: Option<&str>) -> Option<TuiDisplayMode> {
    let value = value?.trim().to_ascii_lowercase();
    match value.as_str() {
        "standard" | "std" | "unicode" => Some(TuiDisplayMode::Standard),
        // Legacy aliases from the former punctuation-coupled display mode.
        "punctuation" | "punct" | "cjk" | "compatible" | "compat" => Some(TuiDisplayMode::Standard),
        "basic" | "ascii" | "plain" | "safe" => Some(TuiDisplayMode::Basic),
        _ => None,
    }
}

pub fn parse_punctuation_mode(value: Option<&str>) -> Option<TuiPunctuationMode> {
    let value = value?.trim().to_ascii_lowercase();
    match value.as_str() {
        "preserve" | "off" | "none" | "raw" => Some(TuiPunctuationMode::Preserve),
        "colon-comma"
        | "colon_comma"
        | "coloncomma"
        | "colon-comma-semicolon"
        | "basic"
        | "minimal"
        | "cn-basic"
        | "optimized"
        | "optimize"
        | "spacing" => Some(TuiPunctuationMode::ColonComma),
        "common" | "all" | "cjk" | "punctuation" | "punct" | "compatible" | "compat" => {
            Some(TuiPunctuationMode::Common)
        }
        _ => None,
    }
}

pub fn parse_theme(value: Option<&str>) -> Option<TuiTheme> {
    let value = value?.trim().to_ascii_lowercase();
    match value.as_str() {
        "blue" | "standard" => Some(TuiTheme::Blue),
        "orange" | "default" | "dark-orange" | "deep-orange" | "amber" => Some(TuiTheme::Orange),
        _ => None,
    }
}

pub fn normalize_text_for_mode(value: &str) -> String {
    let mode = current_punctuation_mode();
    normalize_text_for_punctuation_mode(value, mode)
}

pub fn normalize_text_for_punctuation_mode(value: &str, mode: TuiPunctuationMode) -> String {
    if matches!(mode, TuiPunctuationMode::Preserve) {
        return value.to_owned();
    }

    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '，' if matches!(mode, TuiPunctuationMode::ColonComma) => {
                push_ascii_punctuation(&mut output, &mut chars, ',')
            }
            '：' if matches!(mode, TuiPunctuationMode::ColonComma) => {
                push_ascii_punctuation(&mut output, &mut chars, ':')
            }
            '；' if matches!(mode, TuiPunctuationMode::ColonComma) => {
                push_ascii_punctuation(&mut output, &mut chars, ';')
            }
            '。' if matches!(mode, TuiPunctuationMode::ColonComma) => {
                push_ascii_punctuation(&mut output, &mut chars, '.')
            }
            '、' if matches!(mode, TuiPunctuationMode::ColonComma) => {
                push_ascii_punctuation(&mut output, &mut chars, ',')
            }
            '！' | '？' | '“' | '”' | '‘' | '’' | '｜' | '／' | '＼' | '－' | '—' | '–' | '…' | '\u{3000}'
                if matches!(mode, TuiPunctuationMode::ColonComma) =>
            {
                output.push(ch)
            }
            '，' => output.push(','),
            '：' => output.push(':'),
            '；' => output.push(';'),
            '。' => output.push('.'),
            '！' => output.push('!'),
            '？' => output.push('?'),
            '（' => output.push('('),
            '）' => output.push(')'),
            '【' | '「' | '『' => output.push('['),
            '】' | '」' | '』' => output.push(']'),
            '《' | '〈' => output.push('<'),
            '》' | '〉' => output.push('>'),
            '“' | '”' => output.push('"'),
            '‘' | '’' => output.push('\''),
            '、' => output.push(','),
            '｜' => output.push('|'),
            '／' => output.push('/'),
            '＼' => output.push('\\'),
            '－' | '—' | '–' => output.push('-'),
            '～' => output.push('~'),
            '…' => output.push_str("..."),
            '\u{3000}' => output.push(' '),
            _ => output.push(ch),
        }
    }
    output
}

fn push_ascii_punctuation(
    output: &mut String,
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    punctuation: char,
) {
    output.push(punctuation);
    if chars.peek().is_some() {
        output.push(' ');
        if matches!(chars.peek(), Some(next) if next.is_whitespace()) {
            chars.next();
        }
    }
}

#[cfg(test)]
mod tests {
    use clash_core::IAppSettings;

    use super::{
        TuiDisplayMode, TuiPunctuationMode, TuiTheme, parse_display_mode, parse_punctuation_mode, parse_theme,
    };

    #[test]
    fn display_mode_parser_accepts_user_facing_aliases() {
        assert_eq!(parse_display_mode(Some("standard")), Some(TuiDisplayMode::Standard));
        assert_eq!(parse_display_mode(Some("cjk")), Some(TuiDisplayMode::Standard));
        assert_eq!(parse_display_mode(Some("ascii")), Some(TuiDisplayMode::Basic));
        assert_eq!(parse_display_mode(Some("unknown")), None);
    }

    #[test]
    fn punctuation_mode_parser_accepts_user_facing_aliases() {
        assert_eq!(
            parse_punctuation_mode(Some("preserve")),
            Some(TuiPunctuationMode::Preserve)
        );
        assert_eq!(
            parse_punctuation_mode(Some("colon-comma")),
            Some(TuiPunctuationMode::ColonComma)
        );
        assert_eq!(parse_punctuation_mode(Some("cjk")), Some(TuiPunctuationMode::Common));
        assert_eq!(parse_punctuation_mode(Some("unknown")), None);
    }

    #[test]
    fn theme_parser_accepts_user_facing_aliases() {
        assert_eq!(parse_theme(Some("blue")), Some(TuiTheme::Blue));
        assert_eq!(parse_theme(Some("default")), Some(TuiTheme::Orange));
        assert_eq!(parse_theme(Some("orange")), Some(TuiTheme::Orange));
        assert_eq!(parse_theme(Some("deep-orange")), Some(TuiTheme::Orange));
        assert_eq!(parse_theme(Some("unknown")), None);
    }

    #[test]
    fn theme_defaults_to_orange_when_unconfigured() {
        let app_settings = IAppSettings::default();
        let summary = super::theme_summary(&app_settings);
        assert_eq!(super::configured_theme(&app_settings), TuiTheme::Orange);
        assert_eq!(summary.configured, "orange");
        assert_eq!(summary.configured_label, "深橙");
        assert_eq!(super::theme_from_summary(&summary), TuiTheme::Orange);
    }

    #[test]
    fn punctuation_mode_controls_replacement_scope() {
        assert_eq!(
            super::normalize_text_for_punctuation_mode("模式：规则，状态。", TuiPunctuationMode::Preserve),
            "模式：规则，状态。"
        );

        assert_eq!(
            super::normalize_text_for_punctuation_mode("模式：规则，状态。", TuiPunctuationMode::Common),
            "模式:规则,状态."
        );

        assert_eq!(
            super::normalize_text_for_punctuation_mode("模式：规则，状态。", TuiPunctuationMode::ColonComma),
            "模式: 规则, 状态."
        );
    }

    #[test]
    fn optimized_punctuation_mode_normalizes_punctuation_spacing() {
        assert_eq!(
            super::normalize_text_for_punctuation_mode("操作： Enter，选择；确认", TuiPunctuationMode::ColonComma),
            "操作: Enter, 选择; 确认"
        );
        assert_eq!(
            super::normalize_text_for_punctuation_mode("句号。下一句、项目", TuiPunctuationMode::ColonComma),
            "句号. 下一句, 项目"
        );

        assert_eq!(
            super::normalize_text_for_punctuation_mode("错误：", TuiPunctuationMode::ColonComma),
            "错误:"
        );
    }

    #[test]
    fn common_punctuation_mode_covers_frequent_symbols() {
        assert_eq!(
            super::normalize_text_for_punctuation_mode(
                "提示：“订阅（默认）”《规则》、状态！",
                TuiPunctuationMode::Common
            ),
            "提示:\"订阅(默认)\"<规则>,状态!"
        );
        assert_eq!(
            super::normalize_text_for_punctuation_mode(
                "提示：“订阅（默认）”《规则》、状态！",
                TuiPunctuationMode::ColonComma
            ),
            "提示: “订阅(默认)”<规则>, 状态！"
        );
    }

    #[test]
    fn optimized_punctuation_mode_preserves_verified_safe_symbols() {
        assert_eq!(
            super::normalize_text_for_punctuation_mode(
                "问号？感叹！引号“中”单引‘中’线条｜／＼破折—–－省略…空格　",
                TuiPunctuationMode::ColonComma
            ),
            "问号？感叹！引号“中”单引‘中’线条｜／＼破折—–－省略…空格　"
        );
    }
}
