//! Deterministic Faker augmentation. Generated prose is never ground truth.

use fake::faker::company::en::CompanyName;
use fake::faker::internet::en::Username;
use fake::{Fake, Faker};
use rand::rngs::StdRng;
use rand::SeedableRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::common::pseudonymous_id;
use super::schema::{CandidateMemory, SourceKind, SESSION_REPLAY_SCHEMA_VERSION};

/// Exact Faker dependency version pinned in the workspace manifest.
pub const FAKER_VERSION: &str = "5.1.0";
/// Reproducible default used unless a local run explicitly preregisters another seed.
pub const DEFAULT_FAKER_SEED: u64 = 0x5255_5354_5942_5241;
/// Controlled corpus sizes consumed by the scale harness follow-up.
pub const DISTRACTOR_CORPUS_SIZES: [usize; 3] = [1_000, 10_000, 25_000];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VariantKind {
    EvidenceFrame,
    SyntheticContextFrame,
}

/// An augmentation-only surface variant derived from a sanitized candidate.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ControlledVariant {
    pub schema_version: String,
    pub variant_id: String,
    pub source_candidate_id: String,
    pub kind: VariantKind,
    pub text: String,
    pub faker_version: String,
    pub faker_seed: u64,
    pub semantic_ground_truth: bool,
}

/// A known-irrelevant structured distractor. Faker supplies identifiers only.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Distractor {
    pub schema_version: String,
    pub distractor_id: String,
    pub source: SourceKind,
    pub corpus_size: usize,
    pub ordinal: usize,
    pub text: String,
    pub faker_version: String,
    pub faker_seed: u64,
    pub relevant: bool,
    pub semantic_ground_truth: bool,
}

/// Generate two fixed-frame variants. Faker never invents the factual payload.
pub fn controlled_variants(candidate: &CandidateMemory, seed: u64) -> Vec<ControlledVariant> {
    let mut rng = StdRng::seed_from_u64(seed ^ stable_candidate_seed(candidate));
    let actor: String = Username().fake_with_rng(&mut rng);
    let project: String = CompanyName().fake_with_rng(&mut rng);
    [
        (
            VariantKind::EvidenceFrame,
            format!(
                "Earlier evidence (synthetic actor {actor}): {}",
                candidate.text
            ),
        ),
        (
            VariantKind::SyntheticContextFrame,
            format!(
                "Synthetic project {project} recorded this prior evidence: {}",
                candidate.text
            ),
        ),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (kind, text))| ControlledVariant {
        schema_version: SESSION_REPLAY_SCHEMA_VERSION.to_string(),
        variant_id: pseudonymous_id(
            "variant",
            seed,
            &[&candidate.candidate_id, &index.to_string()],
        ),
        source_candidate_id: candidate.candidate_id.clone(),
        kind,
        text,
        faker_version: FAKER_VERSION.to_string(),
        faker_seed: seed,
        semantic_ground_truth: false,
    })
    .collect()
}

/// Generate deterministic fixed-template distractors for a requested scale.
///
/// Faker supplies only aliases and numeric dimensions. The sentence templates
/// are authored here and every record is explicitly irrelevant/non-ground-truth.
pub fn generate_distractors(corpus_size: usize, seed: u64) -> Vec<Distractor> {
    let mut rng = StdRng::seed_from_u64(seed ^ corpus_size as u64);
    (0..corpus_size)
        .map(|ordinal| {
            let project: String = CompanyName().fake_with_rng(&mut rng);
            let actor: String = Username().fake_with_rng(&mut rng);
            let shard: u16 = (Faker).fake_with_rng(&mut rng);
            let template = match ordinal % 3 {
                0 => format!(
                    "Synthetic distractor {ordinal}: project {project} assigned actor {actor} to build shard {shard}."
                ),
                1 => format!(
                    "Synthetic distractor {ordinal}: actor {actor} rotated test bucket {shard} for project {project}."
                ),
                _ => format!(
                    "Synthetic distractor {ordinal}: project {project} logged fixture batch {shard} under actor {actor}."
                ),
            };
            Distractor {
                schema_version: SESSION_REPLAY_SCHEMA_VERSION.to_string(),
                distractor_id: pseudonymous_id(
                    "distractor",
                    seed,
                    &[&corpus_size.to_string(), &ordinal.to_string()],
                ),
                source: SourceKind::Synthetic,
                corpus_size,
                ordinal,
                text: template,
                faker_version: FAKER_VERSION.to_string(),
                faker_seed: seed,
                relevant: false,
                semantic_ground_truth: false,
            }
        })
        .collect()
}

fn stable_candidate_seed(candidate: &CandidateMemory) -> u64 {
    let digest = Sha256::digest(candidate.candidate_id.as_bytes());
    let mut seed_bytes = [0u8; 8];
    seed_bytes.copy_from_slice(&digest[..8]);
    u64::from_le_bytes(seed_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_replay::{
        DatasetSplit, EvidenceAuthority, Provenance, ReviewStatus, SourceKind,
    };
    use chrono::TimeZone;

    fn candidate() -> CandidateMemory {
        CandidateMemory {
            schema_version: SESSION_REPLAY_SCHEMA_VERSION.to_string(),
            candidate_id: "candidate-invented".to_string(),
            session_id: "session-invented".to_string(),
            project_id: "project-invented".to_string(),
            timestamp: chrono::Utc
                .timestamp_opt(1_700_000_000, 0)
                .single()
                .unwrap(),
            split: DatasetSplit::Development,
            authority: EvidenceAuthority::UserStatement,
            review_status: ReviewStatus::Unreviewed,
            semantic_ground_truth: false,
            text: "The invented cache uses three shards.".to_string(),
            provenance: Provenance {
                source: SourceKind::Synthetic,
                source_locator_id: "source-invented".to_string(),
                source_record_id: "record-invented".to_string(),
                source_record_index: 0,
                adapter_version: "invented-v1".to_string(),
            },
        }
    }

    #[test]
    fn augmentation_is_deterministic_for_seed() {
        assert_eq!(
            controlled_variants(&candidate(), 9),
            controlled_variants(&candidate(), 9)
        );
    }

    #[test]
    fn candidate_seed_uses_the_complete_candidate_id() {
        let first = candidate();
        let mut second = candidate();
        second.candidate_id = "candidate-invented-second".to_string();

        assert_ne!(
            stable_candidate_seed(&first),
            stable_candidate_seed(&second)
        );
        assert_ne!(
            controlled_variants(&first, 9)[0].text,
            controlled_variants(&second, 9)[0].text
        );
    }

    #[test]
    fn distractor_generation_is_deterministic_and_not_ground_truth() {
        let first = generate_distractors(10, 9);
        let second = generate_distractors(10, 9);
        assert_eq!(first, second);
        assert!(first
            .iter()
            .all(|record| !record.relevant && !record.semantic_ground_truth));
    }

    #[test]
    fn scale_sizes_are_preregistered() {
        assert_eq!(DISTRACTOR_CORPUS_SIZES, [1_000, 10_000, 25_000]);
    }

    #[test]
    fn augmentation_dependency_versions_are_exactly_pinned() {
        const WORKSPACE_MANIFEST: &str = include_str!("../../../../Cargo.toml");
        assert!(WORKSPACE_MANIFEST.contains("fake = \"=5.1.0\""));
        assert!(WORKSPACE_MANIFEST.contains("rand = \"=0.10.2\""));
        assert_eq!(FAKER_VERSION, "5.1.0");
    }
}
