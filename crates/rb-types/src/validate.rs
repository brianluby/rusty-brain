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
}
