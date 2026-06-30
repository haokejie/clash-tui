use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ValidationErrorKind {
    CoreRejected,
    Timeout,
    ProcessTerminated,
    ScriptMissingMain,
    ScriptSyntax,
    YamlMapping,
    YamlSyntax,
    FileRead,
    FileMissing,
}

impl ValidationErrorKind {
    #[must_use]
    pub fn from_message(message: &str) -> Self {
        let lower = message.to_ascii_lowercase();
        VALIDATION_SIGNALS
            .iter()
            .find(|signal| signal.matches(&lower))
            .map_or(Self::CoreRejected, |signal| signal.kind)
    }
}

#[derive(Debug, Clone, Copy)]
struct ValidationSignal {
    kind: ValidationErrorKind,
    needles: &'static [&'static str],
}

impl ValidationSignal {
    fn matches(self, message: &str) -> bool {
        self.needles.iter().any(|needle| message.contains(needle))
    }
}

const VALIDATION_SIGNALS: &[ValidationSignal] = &[
    signal(ValidationErrorKind::FileMissing, &["file not found", "no such file"]),
    signal(ValidationErrorKind::FileRead, &["failed to read", "无法读取"]),
    signal(
        ValidationErrorKind::ScriptMissingMain,
        &["script must contain a main function", "main is not defined"],
    ),
    signal(ValidationErrorKind::ScriptSyntax, &["script syntax error"]),
    signal(
        ValidationErrorKind::YamlMapping,
        &[
            "mapping values are not allowed",
            "failed to transform to yaml mapping",
            "failed to apply merge",
            "yaml root must be a mapping",
        ],
    ),
    signal(
        ValidationErrorKind::YamlSyntax,
        &["yaml syntax error", "failed to parse yaml", "did not find expected key"],
    ),
    signal(ValidationErrorKind::Timeout, &["timeout", "超时"]),
    signal(ValidationErrorKind::ProcessTerminated, &["terminated", "被终止"]),
];

const fn signal(kind: ValidationErrorKind, needles: &'static [&'static str]) -> ValidationSignal {
    ValidationSignal { kind, needles }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ValidationSkipReason {
    Debounced,
    Exiting,
}

impl fmt::Display for ValidationSkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Debounced => "debounced",
            Self::Exiting => "application is exiting",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ValidationOutcome {
    Busy,
    Valid,
    Skipped { reason: ValidationSkipReason },
    Invalid { message: String, kind: ValidationErrorKind },
}

impl ValidationOutcome {
    #[must_use]
    pub fn invalid(kind: ValidationErrorKind, message: impl Into<String>) -> Self {
        let message = message.into();
        Self::Invalid { message, kind }
    }

    #[must_use]
    pub fn invalid_from_message(message: impl Into<String>) -> Self {
        let message = message.into();
        let kind = ValidationErrorKind::from_message(message.as_str());
        Self::Invalid { message, kind }
    }

    #[must_use]
    pub const fn is_valid(&self) -> bool {
        matches!(self, Self::Valid)
    }
}

impl fmt::Display for ValidationOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => f.write_str("Configuration validation is already running"),
            Self::Valid => f.write_str("configuration is valid"),
            Self::Skipped { reason } => write!(f, "Configuration validation skipped: {reason}"),
            Self::Invalid { message, .. } => f.write_str(message),
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
