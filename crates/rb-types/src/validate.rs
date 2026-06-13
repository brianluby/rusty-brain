use crate::error::{Error, Result};

/// The single source of truth for the valid importance range (inclusive 1..=10),
/// matching the `importance INTEGER CHECK (importance BETWEEN 1 AND 10)` schema
/// constraint. Returns [`Error::InvalidArgument`] on a value outside the range so
/// callers report a caller mistake, not a storage fault.
pub fn validate_importance(importance: u8) -> Result<()> {
    if (1..=10).contains(&importance) {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "importance {importance} is out of range 1..=10"
        )))
    }
}

/// The single source of truth for the valid confidence range (inclusive
/// 0.0..=1.0, finite), matching the
/// `confidence REAL CHECK (confidence BETWEEN 0.0 AND 1.0)` schema constraint.
/// Returns [`Error::InvalidArgument`] on NaN/infinite or out-of-range values so
/// callers report a caller mistake, not a storage fault.
pub fn validate_confidence(confidence: f32) -> Result<()> {
    if confidence.is_finite() && (0.0..=1.0).contains(&confidence) {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "confidence {confidence} out of range 0.0..=1.0"
        )))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::Error;

    #[test]
    fn accepts_the_inclusive_1_to_10_range() {
        for imp in 1u8..=10 {
            assert!(validate_importance(imp).is_ok(), "{imp} must be valid");
        }
    }

    #[test]
    fn rejects_below_and_above_range_as_invalid_argument() {
        for bad in [0u8, 11, 255] {
            let err = validate_importance(bad).unwrap_err();
            assert!(
                matches!(err, Error::InvalidArgument(_)),
                "expected InvalidArgument for {bad}, got {err:?}"
            );
            assert!(
                err.to_string().contains("importance")
                    && err.to_string().contains(&bad.to_string()),
                "message must name the field and value: {err}"
            );
        }
    }

    #[test]
    fn confidence_accepts_the_inclusive_0_to_1_range() {
        for ok in [0.0f32, 0.5, 0.7, 1.0] {
            assert!(validate_confidence(ok).is_ok(), "{ok} must be valid");
        }
    }

    #[test]
    fn confidence_rejects_out_of_range_and_non_finite_as_invalid_argument() {
        for bad in [-0.1f32, 1.1, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let err = validate_confidence(bad).unwrap_err();
            assert!(
                matches!(err, Error::InvalidArgument(_)),
                "expected InvalidArgument for {bad}, got {err:?}"
            );
            assert!(
                err.to_string().contains("confidence"),
                "message must name the field: {err}"
            );
        }
    }
}
