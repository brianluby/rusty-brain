//! Compression configuration.

/// Configuration controlling the compression pipeline's behavior.
///
/// All fields have sensible defaults via the `Default` impl.
/// Use `validate()` after construction with custom values to check invariants.
#[derive(Debug, Clone, PartialEq)]
pub struct CompressionConfig {
    /// Character count above which compression is triggered.
    /// Inputs at or below this threshold are returned unchanged.
    /// Default: 3,000.
    pub compression_threshold: usize,

    /// Maximum character count for compressed output.
    /// The final truncation pass guarantees this limit.
    /// Default: 2,000 (~500 tokens).
    pub target_budget: usize,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            compression_threshold: 3_000,
            target_budget: 2_000,
        }
    }
}

impl CompressionConfig {
    /// Validate configuration invariants.
    ///
    /// # Errors
    ///
    /// Returns `Err` with a description if:
    /// - `target_budget` is 0
    /// - `compression_threshold` is 0
    /// - `target_budget >= compression_threshold`
    pub fn validate(&self) -> Result<(), String> {
        if self.target_budget == 0 {
            return Err("target_budget must be > 0".to_string());
        }
        if self.compression_threshold == 0 {
            return Err("compression_threshold must be > 0".to_string());
        }
        if self.target_budget >= self.compression_threshold {
            return Err("target_budget must be < compression_threshold".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_expected_values() {
        let config = CompressionConfig::default();
        assert_eq!(config.compression_threshold, 3_000);
        assert_eq!(config.target_budget, 2_000);
    }

    #[test]
    fn default_validates_ok() {
        assert!(CompressionConfig::default().validate().is_ok());
    }

    #[test]
    fn validate_rejects_zero_budget() {
        let config = CompressionConfig {
            target_budget: 0,
            ..CompressionConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_zero_threshold() {
        let config = CompressionConfig {
            compression_threshold: 0,
            target_budget: 0,
            ..CompressionConfig::default()
        };
        // Should fail on target_budget == 0 first
        let err = config.validate().unwrap_err();
        assert!(err.contains("target_budget"));
    }

    #[test]
    fn validate_rejects_budget_equal_to_threshold() {
        let config = CompressionConfig {
            compression_threshold: 2_000,
            target_budget: 2_000,
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_budget_greater_than_threshold() {
        let config = CompressionConfig {
            compression_threshold: 1_000,
            target_budget: 2_000,
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_accepts_valid_custom_values() {
        let config = CompressionConfig {
            compression_threshold: 5_000,
            target_budget: 3_000,
        };
        assert!(config.validate().is_ok());
    }
}
