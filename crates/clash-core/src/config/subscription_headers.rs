use reqwest::header::{HeaderMap, HeaderValue};

pub(crate) const DEFAULT_USER_AGENT: &str = concat!("clash-verge/v", env!("CLASH_TUI_APP_VERSION"));
pub(crate) const SUBSCRIPTION_USERINFO: &str = "subscription-userinfo";
pub(crate) const PROFILE_UPDATE_INTERVAL: &str = "profile-update-interval";
pub(crate) const PROFILE_WEB_PAGE_URL: &str = "profile-web-page-url";
pub(crate) const PARAM_UPLOAD: &str = "upload";
pub(crate) const PARAM_DOWNLOAD: &str = "download";
pub(crate) const PARAM_TOTAL: &str = "total";
pub(crate) const PARAM_EXPIRE: &str = "expire";

#[must_use]
pub(crate) fn header_value<'a>(headers: &'a HeaderMap, name: &'static str) -> Option<&'a HeaderValue> {
    headers.get(name)
}

#[must_use]
pub(crate) fn header_text(headers: &HeaderMap, name: &'static str) -> Option<String> {
    header_value(headers, name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[must_use]
pub(crate) fn subscription_userinfo_text(headers: &HeaderMap) -> Option<&str> {
    headers.iter().find_map(|(name, value)| {
        let header = name.as_str().to_ascii_lowercase();
        let prefix = header.strip_suffix(SUBSCRIPTION_USERINFO)?;
        if prefix.is_empty() || prefix.ends_with('-') {
            value.to_str().ok()
        } else {
            None
        }
    })
}
