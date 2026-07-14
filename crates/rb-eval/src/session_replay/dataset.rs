//! Chronological candidate/query construction with whole-session splits.

use chrono::{DateTime, Utc};

use super::schema::{
    CandidateDataset, CandidateMemory, DatasetSplit, EventKind, EventRole, NaturalQuery,
    NormalizedEvent, NormalizedSession, ReplayLane, ReviewStatus, SESSION_REPLAY_SCHEMA_VERSION,
};

/// Select the dialogue-only or full-event view without reclassifying tool text.
pub fn events_for_lane(session: &NormalizedSession, lane: ReplayLane) -> Vec<&NormalizedEvent> {
    match lane {
        ReplayLane::DialogueOnly => session
            .events
            .iter()
            .filter(|event| event.is_dialogue())
            .collect(),
        ReplayLane::FullEvent => session.events.iter().collect(),
    }
}

/// Derive a deterministic 80% chronological boundary from session starts.
///
/// Callers may instead preregister and pass an explicit boundary. Whichever
/// boundary is used, [`build_candidate_dataset`] never splits a session.
pub fn derive_holdout_boundary(sessions: &[NormalizedSession]) -> Option<DateTime<Utc>> {
    let mut starts: Vec<_> = sessions.iter().map(|session| session.started_at).collect();
    starts.sort_unstable();
    let index = starts.len().saturating_mul(4) / 5;
    starts
        .get(index.min(starts.len().saturating_sub(1)))
        .copied()
}

/// Build natural later-user queries and the earlier authoritative candidate pool.
///
/// Assistant text is preserved in the dialogue lane but its authority class is
/// ineligible here. Automatic refer-back detection only proposes unreviewed
/// local records; no relevance label becomes semantic ground truth.
pub fn build_candidate_dataset(
    sessions: &[NormalizedSession],
    holdout_boundary: DateTime<Utc>,
) -> CandidateDataset {
    let mut ordered_sessions: Vec<_> = sessions.iter().collect();
    ordered_sessions.sort_by(|left, right| {
        (left.started_at, &left.session_id).cmp(&(right.started_at, &right.session_id))
    });

    let mut candidates = Vec::new();
    let mut queries = Vec::new();
    let mut development_sessions = 0u64;
    let mut holdout_sessions = 0u64;
    let mut crossing_sessions_rejected = 0u64;

    for session in ordered_sessions {
        let split = if session.ended_at < holdout_boundary {
            development_sessions += 1;
            DatasetSplit::Development
        } else if session.started_at >= holdout_boundary {
            holdout_sessions += 1;
            DatasetSplit::Holdout
        } else {
            crossing_sessions_rejected += 1;
            continue;
        };

        let mut prior_candidate_ids = Vec::new();
        for event in &session.events {
            if is_natural_refer_back_query(event) && !prior_candidate_ids.is_empty() {
                queries.push(NaturalQuery {
                    schema_version: SESSION_REPLAY_SCHEMA_VERSION.to_string(),
                    query_id: format!("query-{}", event.event_id),
                    session_id: event.session_id.clone(),
                    project_id: event.project_id.clone(),
                    timestamp: event.timestamp,
                    split,
                    review_status: ReviewStatus::Unreviewed,
                    semantic_ground_truth: false,
                    text: event.content.clone().unwrap_or_default(),
                    // Newest-first makes later user/tool/repository corrections
                    // precede older claims without inventing a relevance label.
                    candidate_pool_ids: prior_candidate_ids.iter().rev().cloned().collect(),
                    provenance: event.provenance.clone(),
                });
            }

            if event.authority.candidate_eligible() {
                let Some(text) = candidate_text(event) else {
                    continue;
                };
                if text.trim().is_empty() {
                    continue;
                }
                let candidate_id = format!("candidate-{}", event.event_id);
                prior_candidate_ids.push(candidate_id.clone());
                candidates.push(CandidateMemory {
                    schema_version: SESSION_REPLAY_SCHEMA_VERSION.to_string(),
                    candidate_id,
                    session_id: event.session_id.clone(),
                    project_id: event.project_id.clone(),
                    timestamp: event.timestamp,
                    split,
                    authority: event.authority,
                    review_status: ReviewStatus::Unreviewed,
                    semantic_ground_truth: false,
                    text,
                    provenance: event.provenance.clone(),
                });
            }
        }
    }

    CandidateDataset {
        candidates,
        queries,
        development_sessions,
        holdout_sessions,
        crossing_sessions_rejected,
    }
}

fn candidate_text(event: &NormalizedEvent) -> Option<String> {
    if event.kind == EventKind::Dialogue {
        return event.content.clone();
    }
    event.tool.as_ref().and_then(|tool| {
        tool.output
            .as_ref()
            .filter(|value| !value.trim().is_empty())
            .or(tool.input.as_ref())
            .cloned()
    })
}

fn is_natural_refer_back_query(event: &NormalizedEvent) -> bool {
    if event.kind != EventKind::Dialogue || event.role != EventRole::User {
        return false;
    }
    let Some(text) = event.content.as_deref() else {
        return false;
    };
    contains_refer_back_marker(text)
}

fn contains_refer_back_marker(text: &str) -> bool {
    let lowercase = text.to_ascii_lowercase();
    const PHRASE_MARKERS: &[&str] = &[
        "earlier",
        "previously",
        "last time",
        "where we left off",
        "continue from",
        "we decided",
        "did we decide",
        "what did we",
        "remind me",
        "do you remember",
        "refer back",
        "same approach",
    ];
    PHRASE_MARKERS
        .iter()
        .any(|marker| lowercase.contains(marker))
        || lowercase
            .split(|character: char| !character.is_alphanumeric())
            .any(|word| word == "again")
}

#[cfg(test)]
mod tests {
    use super::contains_refer_back_marker;

    #[test]
    fn again_marker_matches_only_a_complete_word() {
        assert!(contains_refer_back_marker("Please try that again."));
        assert!(!contains_refer_back_marker(
            "Guard against accidental leakage."
        ));
    }
}
