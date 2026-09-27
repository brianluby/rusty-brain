//! LoCoMo adapter (Vikunja #60), MemPalace raw session protocol: one
//! document per session of `{speaker} said, "{text}"` lines, evidence dialog
//! ids (`D3:7`) mapped to `session_3`, recall = fraction of evidence sessions
//! in the top k. Unlike MemPalace (one collection per conversation), every
//! question gets a fresh store so recall-side state such as access counts
//! cannot leak between questions; the `Memo` provider keeps that cheap.

use super::core::{retrieve, Hit, Profile};
use super::metrics::{score, QuestionMetrics};
use rb_embed::EmbeddingProvider;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

#[derive(Debug, Clone, Deserialize)]
pub struct Dialog {
    pub speaker: String,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Qa {
    pub question: String,
    pub category: u8,
    #[serde(default)]
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Sample {
    pub sample_id: String,
    pub conversation: serde_json::Map<String, serde_json::Value>,
    pub qa: Vec<Qa>,
}

/// Session documents in session order; ids are `session_N`.
pub fn session_documents(sample: &Sample) -> rb_types::Result<Vec<(String, String)>> {
    let mut docs = Vec::new();
    for n in 1.. {
        let Some(value) = sample.conversation.get(&format!("session_{n}")) else {
            break;
        };
        let dialogs: Vec<Dialog> = serde_json::from_value(value.clone())
            .map_err(|e| rb_types::Error::InvalidArgument(format!("session_{n}: {e}")))?;
        let doc = dialogs
            .iter()
            .map(|d| format!("{} said, \"{}\"", d.speaker, d.text))
            .collect::<Vec<_>>()
            .join("\n");
        docs.push((format!("session_{n}"), doc));
    }
    Ok(docs)
}

pub fn evidence_sessions(evidence: &[String]) -> Vec<String> {
    evidence
        .iter()
        .filter_map(|e| {
            let rest = e.trim().strip_prefix('D')?;
            let (n, _) = rest.split_once(':')?;
            n.parse::<u32>().ok().map(|n| format!("session_{n}"))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// MemPalace's evidence recall: found / |evidence|, and 1.0 when the
/// evidence set is empty (reported separately via `evidence_empty`).
pub fn mempalace_recall(ranked: &[String], evidence: &[String], k: usize) -> f64 {
    if evidence.is_empty() {
        return 1.0;
    }
    let top: BTreeSet<&str> = ranked.iter().take(k).map(String::as_str).collect();
    evidence.iter().filter(|e| top.contains(e.as_str())).count() as f64 / evidence.len() as f64
}

#[derive(Debug, Clone, Serialize)]
pub struct QaRecord {
    pub sample_id: String,
    pub qa_index: usize,
    pub category: u8,
    pub evidence_sessions: Vec<String>,
    pub evidence_empty: bool,
    pub corpus_size: usize,
    pub top_k: usize,
    pub ranked: Vec<Hit>,
    pub abstained: Option<String>,
    pub evidence_recall: f64,
    pub metrics: QuestionMetrics,
    pub ingest_ms: u128,
    pub query_us: u128,
}

pub async fn run_sample<P: EmbeddingProvider>(
    provider: &Arc<P>,
    sample: &Sample,
    profile: Profile,
    top_k: usize,
) -> rb_types::Result<Vec<QaRecord>> {
    let docs = session_documents(sample)?;
    if top_k > docs.len() {
        return Err(rb_types::Error::InvalidArgument(format!(
            "top_k {top_k} exceeds {} candidate sessions in {}",
            docs.len(),
            sample.sample_id
        )));
    }
    let mut out = Vec::with_capacity(sample.qa.len());
    for (qa_index, qa) in sample.qa.iter().enumerate() {
        let r = retrieve(provider, &docs, &qa.question, profile, top_k).await?;
        let ids: Vec<String> = r.ranked.iter().map(|h| h.session_id.clone()).collect();
        let evidence = evidence_sessions(&qa.evidence);
        out.push(QaRecord {
            sample_id: sample.sample_id.clone(),
            qa_index,
            category: qa.category,
            evidence_empty: evidence.is_empty(),
            evidence_recall: mempalace_recall(&ids, &evidence, top_k),
            metrics: score(&ids, &evidence),
            evidence_sessions: evidence,
            corpus_size: docs.len(),
            top_k,
            ranked: r.ranked,
            abstained: r.abstained,
            ingest_ms: r.ingest_ms,
            query_us: r.query_us,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_maps_dialog_ids_to_unique_sessions() {
        let ev = ["D1:3", "D1:9", "D12:1", "bogus"].map(String::from);
        assert_eq!(evidence_sessions(&ev), vec!["session_1", "session_12"]);
    }

    #[test]
    fn empty_evidence_scores_one_like_mempalace() {
        assert_eq!(mempalace_recall(&["session_1".into()], &[], 10), 1.0);
        let ev = vec!["session_1".to_string(), "session_2".to_string()];
        assert_eq!(mempalace_recall(&["session_2".into()], &ev, 10), 0.5);
    }
}
