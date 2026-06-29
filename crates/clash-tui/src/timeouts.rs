use std::time::Duration;

pub(crate) const RUNTIME_RELOAD_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const RUNTIME_RELOAD_CONTROLLER_READY_TIMEOUT: Duration = Duration::from_secs(8);
pub(crate) const START_CORE_RELOAD_CONTROLLER_READY_TIMEOUT: Duration = Duration::from_secs(20);
pub(crate) const RUNTIME_RELOAD_CONTROLLER_READY_INTERVAL: Duration = Duration::from_millis(250);

pub(crate) const SAVED_PROXY_SELECTION_APPLY_TIMEOUT: Duration = Duration::from_secs(8);
pub(crate) const SAVED_PROXY_SELECTION_RETRY_INITIAL: Duration = Duration::from_millis(250);
pub(crate) const SAVED_PROXY_SELECTION_RETRY_SECOND: Duration = Duration::from_millis(500);
pub(crate) const SAVED_PROXY_SELECTION_RETRY_STEADY: Duration = Duration::from_secs(1);

pub(crate) const SYSTEMD_SETTLE_DELAY: Duration = Duration::from_millis(250);

pub(crate) const fn saved_proxy_selection_retry_delay(attempt: usize) -> Duration {
    match attempt {
        0 => SAVED_PROXY_SELECTION_RETRY_INITIAL,
        1 => SAVED_PROXY_SELECTION_RETRY_SECOND,
        _ => SAVED_PROXY_SELECTION_RETRY_STEADY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_proxy_selection_retry_delay_backs_off_to_one_second() {
        assert_eq!(saved_proxy_selection_retry_delay(0), Duration::from_millis(250));
        assert_eq!(saved_proxy_selection_retry_delay(1), Duration::from_millis(500));
        assert_eq!(saved_proxy_selection_retry_delay(2), Duration::from_secs(1));
        assert_eq!(saved_proxy_selection_retry_delay(8), Duration::from_secs(1));
    }
}
