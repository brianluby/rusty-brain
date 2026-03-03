//! Core types for the compression pipeline.

use std::fmt;

/// The tool type, determined by case-insensitive name matching.
///
/// `From<&str>` converts tool names case-insensitively:
/// "read", "Read", "READ" all become `ToolType::Read`.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolType {
    Read,
    Bash,
    Grep,
    Glob,
    Edit,
    Write,
    /// Any tool name not matching the known variants.
    Other(String),
}

impl From<&str> for ToolType {
    fn from(tool_name: &str) -> Self {
        match tool_name.to_ascii_lowercase().as_str() {
            "read" => Self::Read,
            "bash" => Self::Bash,
            "grep" => Self::Grep,
            "glob" => Self::Glob,
            "edit" => Self::Edit,
            "write" => Self::Write,
            other => Self::Other(other.to_string()),
        }
    }
}

impl fmt::Display for ToolType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read => write!(f, "Read"),
            Self::Bash => write!(f, "Bash"),
            Self::Grep => write!(f, "Grep"),
            Self::Glob => write!(f, "Glob"),
            Self::Edit => write!(f, "Edit"),
            Self::Write => write!(f, "Write"),
            Self::Other(name) => write!(f, "Other({name})"),
        }
    }
}

/// Result of a compression operation.
///
/// This type is always returned — the compression API is infallible.
/// When `compression_applied` is `false`, `text` contains the original input unchanged.
#[derive(Debug, Clone, PartialEq)]
pub struct CompressedResult {
    /// The (possibly compressed) output text.
    /// Guaranteed: `text.chars().count() <= config.target_budget` when `compression_applied` is true.
    pub text: String,

    /// Whether compression was actually performed.
    /// `false` when: input is empty, whitespace-only, or below the threshold.
    pub compression_applied: bool,

    /// Character count of the original input (via `.chars().count()`).
    pub original_size: usize,

    /// Compression diagnostics. Present only when `compression_applied` is true.
    pub statistics: Option<CompressionStatistics>,
}

/// Diagnostic data about a compression operation.
///
/// `PartialEq` uses epsilon comparison (1e-9) for `f64` fields (`ratio`, `percentage_saved`)
/// to avoid exact floating-point equality pitfalls.
#[derive(Debug, Clone)]
pub struct CompressionStatistics {
    /// Compression ratio: `original_size / compressed_size`.
    /// Always >= 1.0 when present.
    pub ratio: f64,

    /// Number of characters removed: `original_size - compressed_size`.
    pub chars_saved: usize,

    /// Percentage of original removed: `(chars_saved / original_size) * 100.0`.
    /// Range: 0.0–100.0.
    pub percentage_saved: f64,
}

const F64_EPSILON: f64 = 1e-9;

impl PartialEq for CompressionStatistics {
    fn eq(&self, other: &Self) -> bool {
        self.chars_saved == other.chars_saved
            && (self.ratio - other.ratio).abs() < F64_EPSILON
            && (self.percentage_saved - other.percentage_saved).abs() < F64_EPSILON
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_type_read_lowercase() {
        assert_eq!(ToolType::from("read"), ToolType::Read);
    }

    #[test]
    fn tool_type_read_uppercase() {
        assert_eq!(ToolType::from("READ"), ToolType::Read);
    }

    #[test]
    fn tool_type_read_mixed_case() {
        assert_eq!(ToolType::from("Read"), ToolType::Read);
    }

    #[test]
    fn tool_type_bash() {
        assert_eq!(ToolType::from("Bash"), ToolType::Bash);
    }

    #[test]
    fn tool_type_grep() {
        assert_eq!(ToolType::from("grep"), ToolType::Grep);
    }

    #[test]
    fn tool_type_glob() {
        assert_eq!(ToolType::from("GLOB"), ToolType::Glob);
    }

    #[test]
    fn tool_type_edit() {
        assert_eq!(ToolType::from("edit"), ToolType::Edit);
    }

    #[test]
    fn tool_type_write() {
        assert_eq!(ToolType::from("write"), ToolType::Write);
    }

    #[test]
    fn tool_type_unknown_stores_lowercased() {
        assert_eq!(
            ToolType::from("CustomTool"),
            ToolType::Other("customtool".to_string())
        );
    }

    #[test]
    fn tool_type_unknown_preserves_unknown_name() {
        assert_eq!(
            ToolType::from("WebFetch"),
            ToolType::Other("webfetch".to_string())
        );
    }

    #[test]
    fn tool_type_display() {
        assert_eq!(ToolType::Read.to_string(), "Read");
        assert_eq!(ToolType::Bash.to_string(), "Bash");
        assert_eq!(
            ToolType::Other("custom".to_string()).to_string(),
            "Other(custom)"
        );
    }

    #[test]
    fn statistics_epsilon_equality() {
        let a = CompressionStatistics {
            ratio: 2.0,
            chars_saved: 100,
            percentage_saved: 50.0,
        };
        let b = CompressionStatistics {
            ratio: 2.0 + 1e-12,
            chars_saved: 100,
            percentage_saved: 50.0 - 1e-12,
        };
        assert_eq!(a, b);
    }

    #[test]
    fn statistics_different_values_not_equal() {
        let a = CompressionStatistics {
            ratio: 2.0,
            chars_saved: 100,
            percentage_saved: 50.0,
        };
        let b = CompressionStatistics {
            ratio: 3.0,
            chars_saved: 100,
            percentage_saved: 50.0,
        };
        assert_ne!(a, b);
    }
}
