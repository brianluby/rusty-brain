//! Aggregate-only reporting and permission-restricted local artifact writes.

use std::io::{BufWriter, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::augmentation::{
    controlled_variants, generate_distractors, DISTRACTOR_CORPUS_SIZES, FAKER_VERSION,
};
use super::schema::{
    AdapterOutput, CandidateDataset, EventRole, ImportStats, NormalizedSession, SourceKind,
    SESSION_REPLAY_SCHEMA_VERSION,
};

/// Aggregate dry-run report. It intentionally has no path, hostname, timestamp,
/// transcript, candidate-text, or query-text fields.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InventoryReport {
    pub schema_version: String,
    pub mode: String,
    pub faker_version: String,
    pub faker_seed: u64,
    pub sources: Vec<ImportStats>,
    pub normalized_sessions: u64,
    pub normalized_events: u64,
    pub dialogue_lane_events: u64,
    pub full_event_lane_events: u64,
    pub assistant_events_ineligible_as_truth: u64,
    pub development_sessions: u64,
    pub holdout_sessions: u64,
    pub crossing_sessions_rejected: u64,
    pub candidate_memories: u64,
    pub natural_queries: u64,
    pub semantic_ground_truth_records: u64,
    pub planned_distractor_corpora: [usize; 3],
}

#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error("local replay output directory must be named session-replay-local")]
    UnsafeOutputDirectory,
    #[error("local replay artifact I/O failed")]
    Io(#[from] std::io::Error),
    #[error("local replay artifact serialization failed")]
    Json(#[from] serde_json::Error),
}

/// Build an aggregate-only report from adapter and candidate counts.
pub fn build_inventory_report(
    outputs: &[&AdapterOutput],
    dataset: &CandidateDataset,
    faker_seed: u64,
) -> InventoryReport {
    let mut sources: Vec<_> = outputs.iter().map(|output| output.stats.clone()).collect();
    sources.sort_by_key(|stats| stats.source);
    let normalized_sessions = outputs
        .iter()
        .map(|output| output.sessions.len() as u64)
        .sum();
    let normalized_events = outputs
        .iter()
        .flat_map(|output| &output.sessions)
        .map(|session| session.events.len() as u64)
        .sum();
    let dialogue_lane_events = outputs
        .iter()
        .flat_map(|output| &output.sessions)
        .flat_map(|session| &session.events)
        .filter(|event| event.is_dialogue())
        .count() as u64;
    let assistant_events_ineligible_as_truth = outputs
        .iter()
        .flat_map(|output| &output.sessions)
        .flat_map(|session| &session.events)
        .filter(|event| event.role == EventRole::Assistant)
        .count() as u64;

    InventoryReport {
        schema_version: SESSION_REPLAY_SCHEMA_VERSION.to_string(),
        mode: "aggregate_dry_run".to_string(),
        faker_version: FAKER_VERSION.to_string(),
        faker_seed,
        sources,
        normalized_sessions,
        normalized_events,
        dialogue_lane_events,
        full_event_lane_events: normalized_events,
        assistant_events_ineligible_as_truth,
        development_sessions: dataset.development_sessions,
        holdout_sessions: dataset.holdout_sessions,
        crossing_sessions_rejected: dataset.crossing_sessions_rejected,
        candidate_memories: dataset.candidates.len() as u64,
        natural_queries: dataset.queries.len() as u64,
        semantic_ground_truth_records: 0,
        planned_distractor_corpora: DISTRACTOR_CORPUS_SIZES,
    }
}

/// Write local-only normalized lanes, candidates, variants, distractors, and inventory.
///
/// The final directory name is enforced so the repository ignore rule cannot be
/// bypassed accidentally. Unix artifacts are owner-only; other platforms fail
/// before creating the output directory because equivalent permissions are not
/// available through Rust's standard filesystem API.
pub fn write_local_artifacts(
    output_dir: &Path,
    sessions: &[NormalizedSession],
    dataset: &CandidateDataset,
    report: &InventoryReport,
) -> Result<(), ArtifactError> {
    if output_dir.file_name().and_then(|name| name.to_str()) != Some("session-replay-local") {
        return Err(ArtifactError::UnsafeOutputDirectory);
    }
    create_private_dir(output_dir)?;

    let mut ordered_sessions: Vec<_> = sessions.iter().collect();
    ordered_sessions.sort_by(|left, right| {
        (left.started_at, &left.session_id).cmp(&(right.started_at, &right.session_id))
    });
    let dialogue_events: Vec<_> = ordered_sessions
        .iter()
        .flat_map(|session| &session.events)
        .filter(|event| event.is_dialogue())
        .collect();
    let full_events: Vec<_> = ordered_sessions
        .iter()
        .flat_map(|session| &session.events)
        .collect();
    let session_manifest: Vec<_> = ordered_sessions
        .iter()
        .map(|session| SessionManifest::from(*session))
        .collect();
    let variants: Vec<_> = dataset
        .candidates
        .iter()
        .flat_map(|candidate| controlled_variants(candidate, report.faker_seed))
        .collect();

    write_jsonl(&output_dir.join("sessions.jsonl"), &session_manifest)?;
    write_jsonl(&output_dir.join("dialogue.jsonl"), &dialogue_events)?;
    write_jsonl(&output_dir.join("full-events.jsonl"), &full_events)?;
    write_jsonl(&output_dir.join("candidates.jsonl"), &dataset.candidates)?;
    write_jsonl(&output_dir.join("queries.jsonl"), &dataset.queries)?;
    write_jsonl(&output_dir.join("controlled-variants.jsonl"), &variants)?;
    for size in DISTRACTOR_CORPUS_SIZES {
        let distractors = generate_distractors(size, report.faker_seed);
        write_jsonl(
            &output_dir.join(format!("distractors-{size}.jsonl")),
            &distractors,
        )?;
    }
    write_json(&output_dir.join("inventory.json"), report)?;
    Ok(())
}

#[derive(Serialize)]
struct SessionManifest<'a> {
    schema_version: &'static str,
    session_id: &'a str,
    project_id: &'a str,
    source: SourceKind,
    started_at: chrono::DateTime<chrono::Utc>,
    ended_at: chrono::DateTime<chrono::Utc>,
    event_count: usize,
}

impl<'a> From<&'a NormalizedSession> for SessionManifest<'a> {
    fn from(session: &'a NormalizedSession) -> Self {
        Self {
            schema_version: SESSION_REPLAY_SCHEMA_VERSION,
            session_id: &session.session_id,
            project_id: &session.project_id,
            source: session.source,
            started_at: session.started_at,
            ended_at: session.ended_at,
            event_count: session.events.len(),
        }
    }
}

fn write_jsonl<T: Serialize>(path: &Path, values: &[T]) -> Result<(), ArtifactError> {
    let mut temporary = private_temporary(path)?;
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        for value in values {
            serde_json::to_writer(&mut writer, value)?;
            writer.write_all(b"\n")?;
        }
        writer.flush()?;
    }
    persist_private(temporary, path)?;
    Ok(())
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), ArtifactError> {
    let mut temporary = private_temporary(path)?;
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        serde_json::to_writer_pretty(&mut writer, value)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }
    persist_private(temporary, path)?;
    Ok(())
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> Result<(), std::io::Error> {
    use std::fs::DirBuilder;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    let mut builder = DirBuilder::new();
    builder.recursive(true).mode(0o700).create(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn create_private_dir(_path: &Path) -> Result<(), std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "private local replay artifacts require Unix owner-only permissions",
    ))
}

fn private_temporary(path: &Path) -> Result<tempfile::NamedTempFile, std::io::Error> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "artifact has no parent")
    })?;
    tempfile::NamedTempFile::new_in(parent)
}

fn persist_private(temporary: tempfile::NamedTempFile, path: &Path) -> Result<(), std::io::Error> {
    temporary.as_file().sync_all()?;
    let file = temporary.persist(path).map_err(|error| error.error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::collections::BTreeMap;

    #[cfg(unix)]
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::session_replay::DEFAULT_FAKER_SEED;

    fn empty_report() -> InventoryReport {
        InventoryReport {
            schema_version: SESSION_REPLAY_SCHEMA_VERSION.to_string(),
            mode: "aggregate_dry_run".to_string(),
            faker_version: FAKER_VERSION.to_string(),
            faker_seed: DEFAULT_FAKER_SEED,
            sources: Vec::new(),
            normalized_sessions: 0,
            normalized_events: 0,
            dialogue_lane_events: 0,
            full_event_lane_events: 0,
            assistant_events_ineligible_as_truth: 0,
            development_sessions: 0,
            holdout_sessions: 0,
            crossing_sessions_rejected: 0,
            candidate_memories: 0,
            natural_queries: 0,
            semantic_ground_truth_records: 0,
            planned_distractor_corpora: DISTRACTOR_CORPUS_SIZES,
        }
    }

    fn empty_dataset() -> CandidateDataset {
        CandidateDataset {
            candidates: Vec::new(),
            queries: Vec::new(),
            development_sessions: 0,
            holdout_sessions: 0,
            crossing_sessions_rejected: 0,
        }
    }

    #[cfg(unix)]
    fn file_hashes(directory: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut hashes = BTreeMap::new();
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let bytes = std::fs::read(entry.path()).unwrap();
            hashes.insert(
                entry.file_name().to_string_lossy().into_owned(),
                Sha256::digest(bytes).to_vec(),
            );
        }
        hashes
    }

    #[test]
    fn default_seed_is_reportable() {
        assert_ne!(DEFAULT_FAKER_SEED, 0);
    }

    #[test]
    fn output_directory_name_is_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let error =
            write_local_artifacts(temp.path(), &[], &empty_dataset(), &empty_report()).unwrap_err();
        assert!(matches!(error, ArtifactError::UnsafeOutputDirectory));
    }

    #[cfg(unix)]
    #[test]
    fn local_artifacts_are_deterministic_and_private() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("session-replay-local");
        let dataset = empty_dataset();
        let report = empty_report();

        write_local_artifacts(&output, &[], &dataset, &report).unwrap();
        let first = file_hashes(&output);
        write_local_artifacts(&output, &[], &dataset, &report).unwrap();
        assert_eq!(file_hashes(&output), first);

        for size in DISTRACTOR_CORPUS_SIZES {
            let rows = std::fs::read_to_string(output.join(format!("distractors-{size}.jsonl")))
                .unwrap()
                .lines()
                .count();
            assert_eq!(rows, size);
        }

        use std::os::unix::fs::PermissionsExt;

        assert_eq!(
            std::fs::metadata(&output).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(std::fs::read_dir(&output).unwrap().all(|entry| {
            entry
                .ok()
                .and_then(|entry| entry.metadata().ok())
                .is_some_and(|metadata| metadata.permissions().mode() & 0o777 == 0o600)
        }));
    }

    #[cfg(not(unix))]
    #[test]
    fn local_artifacts_fail_closed_before_creating_output() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("session-replay-local");

        let error =
            write_local_artifacts(&output, &[], &empty_dataset(), &empty_report()).unwrap_err();
        assert!(matches!(
            error,
            ArtifactError::Io(source) if source.kind() == std::io::ErrorKind::Unsupported
        ));
        assert!(!output.exists());
    }
}
