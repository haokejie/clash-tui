use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KernelState {
    #[default]
    Stopped,
    Starting,
    Running,
    Stopping,
    Restarting,
    Crashed,
    Unhealthy,
    Updating,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KernelOwner {
    #[default]
    Stopped,
    Detached,
    Supervised,
    Systemd,
    External,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KernelSnapshot {
    pub state: KernelState,
    #[serde(default)]
    pub owner: KernelOwner,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_exit: Option<String>,
}

impl KernelSnapshot {
    #[must_use]
    pub const fn stopped() -> Self {
        Self {
            state: KernelState::Stopped,
            owner: KernelOwner::Stopped,
            owner_detail: None,
            pid: None,
            version: None,
            last_error: None,
            last_exit: None,
        }
    }
}

impl Default for KernelSnapshot {
    fn default() -> Self {
        Self::stopped()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationStatus {
    pub accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_job: Option<String>,
    pub state: KernelState,
    #[serde(default)]
    pub owner: KernelOwner,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{KernelOwner, KernelSnapshot, KernelState};

    #[test]
    fn kernel_state_uses_api_wire_names() {
        let value = serde_json::to_value(KernelState::Unhealthy).expect("serialize kernel state");
        assert_eq!(value, "unhealthy");

        let snapshot = serde_json::to_value(KernelSnapshot::stopped()).expect("serialize snapshot");
        assert_eq!(snapshot["state"], "stopped");
        assert_eq!(snapshot["owner"], "stopped");
        assert!(snapshot.get("pid").is_none());

        let owner = serde_json::to_value(KernelOwner::Systemd).expect("serialize kernel owner");
        assert_eq!(owner, "systemd");
    }
}
