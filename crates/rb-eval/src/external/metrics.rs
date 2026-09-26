//! Retrieval metrics over an ordered list of retrieved source ids.
//!
//! Two NDCG variants are reported on purpose. `ndcg_mempalace` reproduces the
//! pinned MemPalace runner, whose ideal DCG is built from the relevances of
//! the *retrieved* top-k only (so a run that retrieves one of three gold
//! sessions at rank 1 scores 1.0). `ndcg` is the standard form whose ideal
//! places `min(|gold|, k)` relevant items first. Only the former is
//! comparable with MemPalace's published numbers.

use serde::Serialize;
use std::collections::{BTreeMap, HashSet};

pub const KS: [usize; 4] = [1, 3, 5, 10];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AtK {
    pub recall_any: f64,
    pub recall_all: f64,
    pub ndcg: f64,
    pub ndcg_mempalace: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuestionMetrics {
    /// Keyed by k (serialized as "1", "3", ...).
    pub at: BTreeMap<usize, AtK>,
    /// Reciprocal rank of the first gold id anywhere in the returned list.
    pub mrr: f64,
}

fn dcg(relevances: &[f64]) -> f64 {
    relevances
        .iter()
        .enumerate()
        .map(|(i, rel)| rel / ((i + 2) as f64).log2())
        .sum()
}

pub fn score(ranked: &[String], gold: &[String]) -> QuestionMetrics {
    let gold_set: HashSet<&str> = gold.iter().map(String::as_str).collect();
    let rel: Vec<f64> = ranked
        .iter()
        .map(|id| {
            if gold_set.contains(id.as_str()) {
                1.0
            } else {
                0.0
            }
        })
        .collect();
    let mut at = BTreeMap::new();
    for k in KS {
        let top = &rel[..rel.len().min(k)];
        let top_ids: HashSet<&str> = ranked.iter().take(k).map(String::as_str).collect();
        let recall_any = f64::from(u8::from(gold_set.iter().any(|g| top_ids.contains(g))));
        let recall_all = f64::from(u8::from(
            !gold_set.is_empty() && gold_set.iter().all(|g| top_ids.contains(g)),
        ));
        let mut sorted = top.to_vec();
        sorted.sort_by(|a, b| b.total_cmp(a));
        let idcg_mp = dcg(&sorted);
        let ndcg_mempalace = if idcg_mp == 0.0 {
            0.0
        } else {
            dcg(top) / idcg_mp
        };
        let ideal = vec![1.0; gold_set.len().min(k)];
        let idcg = dcg(&ideal);
        let ndcg = if idcg == 0.0 { 0.0 } else { dcg(top) / idcg };
        at.insert(
            k,
            AtK {
                recall_any,
                recall_all,
                ndcg,
                ndcg_mempalace,
            },
        );
    }
    let mrr = rel
        .iter()
        .position(|r| *r > 0.0)
        .map_or(0.0, |i| 1.0 / (i + 1) as f64);
    QuestionMetrics { at, mrr }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn mempalace_ndcg_uses_retrieved_ideal_but_standard_does_not() {
        let m = score(&ids(&["g1", "x", "y"]), &ids(&["g1", "g2", "g3"]));
        let at3 = &m.at[&3];
        assert!((at3.ndcg_mempalace - 1.0).abs() < 1e-12);
        assert!(at3.ndcg < 0.5);
        assert!((at3.recall_any - 1.0).abs() < 1e-12);
        assert!(at3.recall_all.abs() < 1e-12);
        assert!((m.mrr - 1.0).abs() < 1e-12);
    }

    #[test]
    fn miss_scores_zero_and_short_lists_are_honest() {
        let m = score(&ids(&["x"]), &ids(&["g"]));
        for k in KS {
            assert_eq!(m.at[&k].recall_any, 0.0);
            assert_eq!(m.at[&k].ndcg, 0.0);
        }
        assert_eq!(m.mrr, 0.0);
        let m = score(&ids(&["x", "g"]), &ids(&["g"]));
        assert_eq!(m.at[&1].recall_any, 0.0);
        assert_eq!(m.at[&3].recall_all, 1.0);
        assert!((m.mrr - 0.5).abs() < 1e-12);
    }
}
