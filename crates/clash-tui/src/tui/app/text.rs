pub(crate) fn terminal_safe_text(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\n' | '\r' => output.push_str(" <换行> "),
            '\t' => output.push(' '),
            ch if ch.is_control() => output.push(' '),
            ch => output.push(ch),
        }
    }
    output
}

pub(crate) fn terminal_safe_log_text(value: &str) -> String {
    terminal_safe_text(&redact_urls(value, "[链接]"))
}

pub(crate) fn status_history_text(value: &str) -> String {
    terminal_safe_text(&redact_urls(value, "[链接]"))
}

pub(crate) fn validate_subscription_url(value: &str) -> std::result::Result<(), String> {
    let Ok(url) = url::Url::parse(value) else {
        return Err("订阅链接格式不正确".into());
    };
    if !matches!(url.scheme(), "http" | "https") {
        return Err("订阅链接必须以 http:// 或 https:// 开头".into());
    }
    if url.host_str().is_none() {
        return Err("订阅链接缺少主机名".into());
    }
    Ok(())
}

pub(crate) fn normalize_pasted_text(value: &str) -> String {
    value.replace(['\r', '\n'], "")
}

pub(crate) fn pasted_subscription_url(value: &str) -> Option<String> {
    let value = value.trim();
    validate_subscription_url(value).is_ok().then(|| value.to_owned())
}

pub(crate) fn sanitize_url_error(message: &str) -> String {
    let output = redact_urls(message, "[订阅链接]");
    if output.trim().is_empty() {
        "未知错误".into()
    } else {
        output
    }
}

pub(crate) fn profile_update_message_label(message: &str) -> String {
    if message.trim().is_empty() {
        return String::new();
    }
    let sanitized = sanitize_url_error(message);
    match sanitized.trim() {
        "profile updated" | "订阅已更新" => "订阅已更新".into(),
        "profile updated; runtime refreshed" | "订阅已更新；运行配置已刷新" => {
            "订阅已更新；运行配置已刷新".into()
        }
        "profile updated; runtime refreshed; core restarted" | "订阅已更新；运行配置已刷新；核心已重启" => {
            "订阅已更新；运行配置已刷新；核心已重启".into()
        }
        "profile updated; runtime refresh needs attention" | "订阅已更新；运行配置刷新需要处理" => {
            "订阅已更新；运行配置刷新需要处理".into()
        }
        _ => sanitized,
    }
}

pub(crate) fn redact_urls(message: &str, replacement: &str) -> String {
    let mut output = String::with_capacity(message.len());
    let mut remaining = message;
    while let Some((offset, scheme_len)) = next_url_offset(remaining) {
        output.push_str(&remaining[..offset]);
        let after_scheme = offset + scheme_len;
        let tail = &remaining[after_scheme..];
        if tail.is_empty() || tail.starts_with(char::is_whitespace) {
            output.push_str(&remaining[offset..after_scheme]);
            remaining = tail;
            continue;
        }
        output.push_str(replacement);
        let url_len = tail
            .find(|ch: char| ch.is_whitespace() || matches!(ch, '"' | '\'' | ')' | ']' | '}' | ',' | ';'))
            .map_or(remaining.len() - offset, |end| scheme_len + end);
        remaining = &remaining[offset + url_len..];
    }
    output.push_str(remaining);
    output
}

pub(crate) fn next_url_offset(value: &str) -> Option<(usize, usize)> {
    let http = value.find("http://").map(|offset| (offset, "http://".len()));
    let https = value.find("https://").map(|offset| (offset, "https://".len()));
    match (http, https) {
        (Some(http), Some(https)) => Some(if http.0 <= https.0 { http } else { https }),
        (Some(http), None) => Some(http),
        (None, Some(https)) => Some(https),
        (None, None) => None,
    }
}

pub(crate) fn log_has_level(log: &str, levels: &[&str]) -> bool {
    let log = log.to_ascii_lowercase();
    levels.iter().any(|level| {
        log.contains(&format!("level={level}"))
            || log.contains(&format!("[{level}]"))
            || log.contains(&format!("{level}:"))
            || log.contains(&format!(" {level} "))
    })
}
