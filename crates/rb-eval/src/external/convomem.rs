//! ConvoMem adapter (Vikunja #60), MemPalace's published sample protocol:
//! per category, walk the cached HF file list in order and take the first
//! `limit` evidence items; one document per message; top-k retrieval.
//!
//! MemPalace scores by text: an evidence text counts as found when it is a
//! substring of a retrieved message or vice versa (both lowercased and
//! trimmed). Here that predicate runs once over the corpus *before*
//! retrieval to produce gold message ids, and scoring is by id, so the
//! semantics match without matching retrieved text. The reverse direction
//! lets an empty message match everything; such gold ids are counted.

use super::core::{retrieve, Hit, Profile};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};
use std::path::Path;
use std::sync::Arc;

use rb_embed::EmbeddingProvider;

pub const CATEGORIES: [&str; 6] = [
    "user_evidence",
    "assistant_facts_evidence",
    "changing_evidence",
    "abstention_evidence",
    "preference_evidence",
    "implicit_connection_evidence",
];

#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Conversation {
    #[serde(default)]
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Item {
    pub question: String,
    #[serde(default)]
    pub message_evidences: Vec<Message>,
    #[serde(default)]
    pub conversations: Vec<Conversation>,
}

#[derive(Debug, Deserialize)]
struct EvidenceFile {
    #[serde(default)]
    evidence_items: Vec<Item>,
}

/// A selected item plus where it came from, for the audit trail.
#[derive(Debug, Clone)]
pub struct Selected {
    pub category: &'static str,
    pub file: String,
    pub index_in_file: usize,
    pub item: Item,
}

/// Mirror MemPalace's `load_evidence_items` over its on-disk cache layout:
/// `<cache>/<cat>_filelist.json` and `<cache>/<cat>/<subpath with / -> _>`.
/// Returns the items and the files actually read (for pinning).
pub fn select(
    cache: &Path,
    category: &'static str,
    limit: usize,
) -> anyhow::Result<(Vec<Selected>, Vec<String>)> {
    // MemPalace lists only `1_evidence/`; `changing_evidence` has none, so
    // its runner 404s and skips the category. Mirror that skip exactly.
    let list_path = cache.join(format!("{category}_filelist.json"));
    if !list_path.exists() {
        return Ok((Vec::new(), Vec::new()));
    }
    let list: Vec<String> = serde_json::from_slice(&std::fs::read(list_path)?)?;
    let mut out = Vec::new();
    let mut read = Vec::new();
    for file in list {
        if out.len() >= limit {
            break;
        }
        let path = cache.join(category).join(file.replace('/', "_"));
        let parsed: EvidenceFile = serde_json::from_slice(&std::fs::read(&path)?)?;
        for (index_in_file, item) in parsed.evidence_items.into_iter().enumerate() {
            out.push(Selected {
                category,
                file: file.clone(),
                index_in_file,
                item,
            });
        }
        read.push(file);
    }
    out.truncate(limit);
    Ok((out, read))
}

fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}

pub fn documents(item: &Item) -> Vec<(String, String)> {
    item.conversations
        .iter()
        .flat_map(|c| &c.messages)
        .enumerate()
        .map(|(i, m)| (format!("msg_{i}"), m.text.clone()))
        .collect()
}

/// Gold message ids per distinct evidence text, via MemPalace's predicate.
pub fn gold(item: &Item, docs: &[(String, String)]) -> Vec<BTreeSet<String>> {
    let normed: Vec<String> = docs.iter().map(|(_, t)| norm(t)).collect();
    let evidence: BTreeSet<String> = item
        .message_evidences
        .iter()
        .map(|e| norm(&e.text))
        .collect();
    evidence
        .iter()
        .map(|e| {
            docs.iter()
                .zip(&normed)
                .filter(|(_, c)| c.contains(e.as_str()) || e.contains(c.as_str()))
                .map(|((id, _), _)| id.clone())
                .collect()
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct ItemRecord {
    pub category: &'static str,
    pub file: String,
    pub index_in_file: usize,
    pub corpus_size: usize,
    pub evidence_count: usize,
    pub found: usize,
    pub recall: f64,
    /// Gold ids that match only because the message is empty.
    pub degenerate_gold: usize,
    /// Distinct evidence texts that normalize to "". MemPalace's predicate
    /// counts these as found against any retrieval; kept for comparability,
    /// reported so an inflated item is visible.
    pub empty_evidence: usize,
    pub gold: Vec<BTreeSet<String>>,
    pub ranked: Vec<Hit>,
    pub abstained: Option<String>,
    pub ingest_ms: u128,
    pub query_us: u128,
}

pub async fn run_item<P: EmbeddingProvider>(
    provider: &Arc<P>,
    selected: &Selected,
    profile: Profile,
    top_k: usize,
) -> rb_types::Result<ItemRecord> {
    let docs = documents(&selected.item);
    let gold = gold(&selected.item, &docs);
    let empty: HashSet<&str> = docs
        .iter()
        .filter(|(_, t)| norm(t).is_empty())
        .map(|(id, _)| id.as_str())
        .collect();
    let degenerate_gold = gold
        .iter()
        .flatten()
        .filter(|id| empty.contains(id.as_str()))
        .count();
    let empty_evidence = selected
        .item
        .message_evidences
        .iter()
        .map(|e| norm(&e.text))
        .collect::<BTreeSet<_>>()
        .iter()
        .filter(|e| e.is_empty())
        .count();
    let r = retrieve(
        provider,
        &docs,
        &selected.item.question,
        profile,
        top_k.min(docs.len()),
    )
    .await?;
    let top: HashSet<&str> = r.ranked.iter().map(|h| h.session_id.as_str()).collect();
    let found = gold
        .iter()
        .filter(|ids| ids.iter().any(|id| top.contains(id.as_str())))
        .count();
    let recall = if gold.is_empty() {
        1.0
    } else {
        found as f64 / gold.len() as f64
    };
    Ok(ItemRecord {
        category: selected.category,
        file: selected.file.clone(),
        index_in_file: selected.index_in_file,
        corpus_size: docs.len(),
        evidence_count: gold.len(),
        found,
        recall,
        degenerate_gold,
        empty_evidence,
        gold,
        ranked: r.ranked,
        abstained: r.abstained,
        ingest_ms: r.ingest_ms,
        query_us: r.query_us,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(evidence: &[&str], messages: &[&str]) -> Item {
        Item {
            question: "q".into(),
            message_evidences: evidence
                .iter()
                .map(|t| Message { text: (*t).into() })
                .collect(),
            conversations: vec![Conversation {
                messages: messages
                    .iter()
                    .map(|t| Message { text: (*t).into() })
                    .collect(),
            }],
        }
    }

    #[test]
    fn gold_uses_mempalace_bidirectional_substring_predicate() {
        let it = item(
            &["I use GREEN for hot leads.", "i use green for hot leads."],
            &["hello", "  I use green for hot leads. Thanks!", "green", ""],
        );
        let docs = documents(&it);
        let g = gold(&it, &docs);
        assert_eq!(g.len(), 1, "evidence texts dedupe after normalization");
        let ids: Vec<&str> = g[0].iter().map(String::as_str).collect();
        assert_eq!(ids, vec!["msg_1", "msg_2", "msg_3"]);
    }
}
