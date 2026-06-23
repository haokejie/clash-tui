use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ValidationErrorKind {
    FileMissing,
    FileRead,
    YamlSyntax,
    YamlMapping,
    ScriptSyntax,
    ScriptMissingMain,
    CoreRejected,
    ProcessTerminated,
    Timeout,
}

impl ValidationErrorKind {
    #[must_use]
    pub fn from_message(message: &str) -> Self {
        let lower = message.to_ascii_lowercase();

        if lower.contains("file not found") || lower.contains("no such file") {
            Self::FileMissing
        } else if lower.contains("failed to read") || lower.contains("无法读取") {
            Self::FileRead
        } else if lower.contains("script must contain a main function") {
            Self::ScriptMissingMain
        } else if lower.contains("script syntax error") {
            Self::ScriptSyntax
        } else if lower.contains("mapping values are not allowed")
            || lower.contains("failed to transform to yaml mapping")
            || lower.contains("failed to apply merge")
            || lower.contains("yaml root must be a mapping")
        {
            Self::YamlMapping
        } else if lower.contains("yaml syntax error")
            || lower.contains("failed to parse yaml")
            || lower.contains("did not find expected key")
        {
            Self::YamlSyntax
        } else if lower.contains("timeout") || lower.contains("超时") {
            Self::Timeout
        } else if lower.contains("terminated") || lower.contains("被终止") {
            Self::ProcessTerminated
        } else {
            Self::CoreRejected
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ValidationSkipReason {
    Exiting,
    Debounced,
}

impl fmt::Display for ValidationSkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exiting => write!(f, "application is exiting"),
            Self::Debounced => write!(f, "debounced"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ValidationOutcome {
    Valid,
    Invalid { kind: ValidationErrorKind, message: String },
    Skipped { reason: ValidationSkipReason },
    Busy,
}

impl ValidationOutcome {
    #[must_use]
    pub fn invalid(kind: ValidationErrorKind, message: impl Into<String>) -> Self {
        Self::Invalid {
            kind,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn invalid_from_message(message: impl Into<String>) -> Self {
        let message = message.into();
        Self::invalid(ValidationErrorKind::from_message(&message), message)
    }

    #[must_use]
    pub const fn is_valid(&self) -> bool {
        matches!(self, Self::Valid)
    }
}

impl fmt::Display for ValidationOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Valid => write!(f, "configuration is valid"),
            Self::Invalid { message, .. } => write!(f, "{message}"),
            Self::Skipped { reason } => write!(f, "Configuration validation skipped: {reason}"),
            Self::Busy => write!(f, "Configuration validation is already running"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ValidationErrorKind, ValidationOutcome};

    #[test]
    fn validation_error_kind_classifies_common_messages() {
        assert_eq!(
            ValidationErrorKind::from_message("File not found: dns_config.yaml"),
            ValidationErrorKind::FileMissing
        );
        assert_eq!(
            ValidationErrorKind::from_message("YAML syntax error: did not find expected key"),
            ValidationErrorKind::YamlSyntax
        );
        assert_eq!(
            ValidationErrorKind::from_message("validation timeout"),
            ValidationErrorKind::Timeout
        );
    }

    #[test]
    fn invalid_from_message_keeps_message() {
        let outcome = ValidationOutcome::invalid_from_message("YAML syntax error: bad");
        assert!(matches!(
            outcome,
            ValidationOutcome::Invalid {
                kind: ValidationErrorKind::YamlSyntax,
                ..
            }
        ));
        assert_eq!(outcome.to_string(), "YAML syntax error: bad");
    }
}
