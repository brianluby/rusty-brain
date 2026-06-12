//! TOML configuration for the evolution jobs. Every job is disabled by default;
//! a missing or absent config file yields `JobsConfig::default()`. All fields
//! are `serde(default)` so a partial file overrides only the keys it names.

use serde::Deserialize;
use std::path::Path;

/// Top-level config: one section per job. All sections default-construct, so an
/// empty file (or no `[link_decay]` table at all) means "everything disabled".
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct JobsConfig {
    pub link_decay: LinkDecayConfig,
    pub consolidation: ConsolidationConfig,
    pub importance: ImportanceConfig,
}

/// Link-decay tuning. Exponential decay of link `strength` by age, floored.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct LinkDecayConfig {
    pub enabled: bool,
    pub interval_secs: u64,
    pub half_life_days: f64,
    pub floor: f64,
    pub prune_below_floor: bool,
    pub batch_limit: usize,
}

impl Default for LinkDecayConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: 86_400,
            half_life_days: 30.0,
            floor: 0.05,
            prune_below_floor: false,
            batch_limit: 1000,
        }
    }
}

/// Consolidation tuning (used by Part S). Declared here so the config file
/// schema is stable from the first release; the job itself lands in Part S.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct ConsolidationConfig {
    pub enabled: bool,
    pub interval_secs: u64,
    /// Near-duplicate gate in `near_duplicates`' similarity unit: RAW cosine
    /// similarity clamped to `[0, 1]` (`1 - cosine_distance`). See [`Default`]
    /// for the W1.1 re-derivation.
    pub similarity_threshold: f32,
    pub batch_limit: usize,
}

impl Default for ConsolidationConfig {
    /// `similarity_threshold` re-derived for the W1.1 cosine-metric rebuild.
    ///
    /// The documented intent was always "near-identical: cosine similarity
    /// at least 0.95", and since W1.1 the store's similarity unit IS raw
    /// cosine similarity (`1 - cosine_distance`), so 0.95 now means exactly
    /// that (cosine distance at most 0.05). Before the rebuild the same
    /// number was compared against `1 - L2/2`, where it meant `L2 <= 0.1` —
    /// cosine similarity 0.995+ on unit vectors, effectively inert. The
    /// numeral is unchanged; its unit finally matches the documentation.
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: 86_400,
            similarity_threshold: 0.95,
            batch_limit: 200,
        }
    }
}

/// Importance-recalibration tuning (used by Part T). Declared here for a stable
/// config schema; the job itself lands in Part T.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct ImportanceConfig {
    pub enabled: bool,
    pub interval_secs: u64,
    pub access_weight: f64,
    pub recency_weight: f64,
    pub half_life_days: f64,
    pub batch_limit: usize,
}

impl Default for ImportanceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: 86_400,
            access_weight: 0.5,
            recency_weight: 0.5,
            half_life_days: 30.0,
            batch_limit: 1000,
        }
    }
}

impl JobsConfig {
    /// Load config from `path`. `None`, or a path that does not exist, yields the
    /// all-disabled default (jobs are opt-in). A parse error is surfaced as
    /// `Error::InvalidArgument` so a typo in the file fails loudly, not silently.
    pub fn load(path: Option<&Path>) -> rb_types::Result<JobsConfig> {
        let Some(path) = path else {
            return Ok(JobsConfig::default());
        };
        if !path.exists() {
            return Ok(JobsConfig::default());
        }
        let text = std::fs::read_to_string(path).map_err(|e| {
            rb_types::Error::InvalidArgument(format!("read jobs config {}: {e}", path.display()))
        })?;
        toml::from_str(&text).map_err(|e| {
            rb_types::Error::InvalidArgument(format!("parse jobs config {}: {e}", path.display()))
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn default_is_all_disabled_with_documented_values() {
        let cfg = JobsConfig::default();

        assert!(!cfg.link_decay.enabled);
        assert_eq!(cfg.link_decay.interval_secs, 86_400);
        assert!((cfg.link_decay.half_life_days - 30.0).abs() < f64::EPSILON);
        assert!((cfg.link_decay.floor - 0.05).abs() < f64::EPSILON);
        assert!(!cfg.link_decay.prune_below_floor);
        assert_eq!(cfg.link_decay.batch_limit, 1000);

        assert!(!cfg.consolidation.enabled);
        assert_eq!(cfg.consolidation.interval_secs, 86_400);
        // W1.1 re-derivation: 0.95 is now RAW cosine similarity (distance <= 0.05).
        assert!((cfg.consolidation.similarity_threshold - 0.95).abs() < f32::EPSILON);
        assert_eq!(cfg.consolidation.batch_limit, 200);

        assert!(!cfg.importance.enabled);
        assert_eq!(cfg.importance.interval_secs, 86_400);
        assert!((cfg.importance.access_weight - 0.5).abs() < f64::EPSILON);
        assert!((cfg.importance.recency_weight - 0.5).abs() < f64::EPSILON);
        assert!((cfg.importance.half_life_days - 30.0).abs() < f64::EPSILON);
        assert_eq!(cfg.importance.batch_limit, 1000);
    }

    #[test]
    fn partial_toml_overrides_only_named_fields() {
        let toml_src = r#"
[link_decay]
enabled = true
half_life_days = 7.0
prune_below_floor = true
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jobs.toml");
        std::fs::write(&path, toml_src).unwrap();

        let cfg = JobsConfig::load(Some(path.as_path())).unwrap();
        // Overridden:
        assert!(cfg.link_decay.enabled);
        assert!((cfg.link_decay.half_life_days - 7.0).abs() < f64::EPSILON);
        assert!(cfg.link_decay.prune_below_floor);
        // Defaulted (serde(default) per field):
        assert_eq!(cfg.link_decay.interval_secs, 86_400);
        assert!((cfg.link_decay.floor - 0.05).abs() < f64::EPSILON);
        assert_eq!(cfg.link_decay.batch_limit, 1000);
        // Untouched sections still disabled:
        assert!(!cfg.consolidation.enabled);
        assert!(!cfg.importance.enabled);
    }

    #[test]
    fn missing_path_yields_default() {
        let cfg = JobsConfig::load(None).unwrap();
        assert!(!cfg.link_decay.enabled);
        assert_eq!(cfg.link_decay.batch_limit, 1000);
    }

    #[test]
    fn nonexistent_file_yields_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.toml");
        let cfg = JobsConfig::load(Some(path.as_path())).unwrap();
        assert!(!cfg.link_decay.enabled);
    }

    #[test]
    fn malformed_toml_is_invalid_argument() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "this is = = not toml [[[").unwrap();
        let err = JobsConfig::load(Some(path.as_path())).unwrap_err();
        assert!(
            matches!(err, rb_types::Error::InvalidArgument(_)),
            "got {err:?}"
        );
    }
}
