//! Contract version validation against supported major version.

use types::ContractValidationResult;

/// The major version currently supported by the adapter system.
pub const SUPPORTED_CONTRACT_MAJOR: u64 = 1;

/// Validate a contract version string against the supported major version.
///
/// Uses semver parsing. Pre-release and build metadata are ignored.
/// Returns compatible=true if major versions match.
/// Never panics.
#[must_use]
pub fn validate_contract(version_str: &str) -> ContractValidationResult {
    match semver::Version::parse(version_str) {
        Ok(ver) => {
            if ver.major == SUPPORTED_CONTRACT_MAJOR {
                ContractValidationResult {
                    compatible: true,
                    reason: None,
                }
            } else {
                ContractValidationResult {
                    compatible: false,
                    reason: Some("incompatible_contract_major".to_string()),
                }
            }
        }
        Err(_) => ContractValidationResult {
            compatible: false,
            reason: Some("invalid_contract_version".to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Compatible versions
    // -------------------------------------------------------------------------

    #[test]
    fn compatible_version_1_2_3() {
        let result = validate_contract("1.2.3");
        assert!(result.compatible, "1.2.3 must be compatible");
        assert_eq!(
            result.reason, None,
            "reason must be None for compatible version"
        );
    }

    #[test]
    fn compatible_version_1_0_0() {
        let result = validate_contract("1.0.0");
        assert!(result.compatible, "1.0.0 must be compatible");
        assert_eq!(
            result.reason, None,
            "reason must be None for compatible version"
        );
    }

    // -------------------------------------------------------------------------
    // Incompatible versions
    // -------------------------------------------------------------------------

    #[test]
    fn incompatible_version_2_0_0() {
        let result = validate_contract("2.0.0");
        assert!(!result.compatible, "2.0.0 must be incompatible");
        assert_eq!(
            result.reason.as_deref(),
            Some("incompatible_contract_major"),
            "reason must be 'incompatible_contract_major'"
        );
    }

    #[test]
    fn incompatible_version_0_9_0() {
        let result = validate_contract("0.9.0");
        assert!(!result.compatible, "0.9.0 must be incompatible");
        assert_eq!(
            result.reason.as_deref(),
            Some("incompatible_contract_major"),
            "reason must be 'incompatible_contract_major'"
        );
    }

    // -------------------------------------------------------------------------
    // Invalid version strings
    // -------------------------------------------------------------------------

    #[test]
    fn invalid_version_not_a_version() {
        let result = validate_contract("not-a-version");
        assert!(!result.compatible, "non-semver string must be incompatible");
        assert_eq!(
            result.reason.as_deref(),
            Some("invalid_contract_version"),
            "reason must be 'invalid_contract_version'"
        );
    }

    #[test]
    fn invalid_version_empty_string() {
        let result = validate_contract("");
        assert!(!result.compatible, "empty string must be incompatible");
        assert_eq!(
            result.reason.as_deref(),
            Some("invalid_contract_version"),
            "reason must be 'invalid_contract_version'"
        );
    }

    // -------------------------------------------------------------------------
    // Pre-release and build metadata (stripped / ignored)
    // -------------------------------------------------------------------------

    #[test]
    fn compatible_with_prerelease() {
        let result = validate_contract("1.0.0-beta.1+build.42");
        assert!(
            result.compatible,
            "1.0.0-beta.1+build.42 must be compatible (metadata stripped)"
        );
        assert_eq!(
            result.reason, None,
            "reason must be None for compatible version"
        );
    }

    #[test]
    fn compatible_with_rc() {
        let result = validate_contract("1.0.0-rc.1");
        assert!(result.compatible, "1.0.0-rc.1 must be compatible");
        assert_eq!(
            result.reason, None,
            "reason must be None for compatible version"
        );
    }
}
