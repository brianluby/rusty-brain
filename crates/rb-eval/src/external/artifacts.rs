//! Complete frozen artifacts without retrieving, embedding, reranking or changing metrics.
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

fn failure(record: &Value, benchmark: &str) -> Result<&'static str> {
    let abstained = !record["abstained"].is_null();
    let (gold, found) = if benchmark == "locomo10" {
        let gold = record["evidence_sessions"]
            .as_array()
            .context("evidence_sessions")?;
        let ranked = record["ranked"].as_array().context("ranked")?;
        (
            gold.len(),
            gold.iter()
                .filter(|id| ranked.iter().any(|h| &h["session_id"] == *id))
                .count(),
        )
    } else if benchmark == "convomem" {
        (
            record["evidence_count"]
                .as_u64()
                .context("evidence_count")? as usize,
            record["found"].as_u64().context("found")? as usize,
        )
    } else if benchmark == "membench" {
        (
            usize::from(!record["targets"].as_array().context("targets")?.is_empty()),
            usize::from(record["hit"].as_bool().context("hit")?),
        )
    } else {
        bail!("unsupported benchmark {benchmark}");
    };
    Ok(super::core::classify_failure(gold, found, abstained))
}

/// Read-only inputs; create-new outputs prevent overwriting frozen evidence.
/// Native channel scores/DB bytes/hardware were not captured and stay unavailable.
pub fn complete_frozen(runs: &Path, out: &Path) -> Result<()> {
    std::fs::create_dir_all(out)?;
    let mut entries = Vec::new();
    for stem in [
        "longmemeval",
        "locomo-top10",
        "convomem-top10",
        "membench-movie-top5",
    ] {
        for suffix in ["native", "parity", "parity-tok256"] {
            let tag = format!("{stem}-{suffix}");
            let summary_name = format!("{tag}.summary.json");
            let records_name = format!("{tag}.jsonl");
            let summary_bytes = std::fs::read(runs.join(&summary_name))?;
            let summary: Value = serde_json::from_slice(&summary_bytes)?;
            if summary["complete"] != true {
                bail!("{summary_name} is not complete");
            }
            let bytes = std::fs::read(runs.join(&records_name))?;
            let text = std::str::from_utf8(&bytes)?;
            let benchmark = summary["benchmark"].as_str().context("benchmark")?;
            let mut failures: BTreeMap<String, usize> = BTreeMap::new();
            let mut output = std::io::BufWriter::new(
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(out.join(&records_name))?,
            );
            let mut count = 0usize;
            for line in text.lines() {
                let mut record: Value = serde_json::from_str(line)?;
                if record.get("failure").is_none() {
                    record["failure"] = json!(failure(&record, benchmark)?);
                }
                *failures
                    .entry(record["failure"].as_str().context("failure")?.into())
                    .or_default() += 1;
                serde_json::to_writer(&mut output, &record)?;
                output.write_all(b"\n")?;
                count += 1;
            }
            output.flush()?;
            if summary["overall"]["n"].as_u64() != Some(count as u64) {
                bail!("{records_name} count differs from summary");
            }
            entries.push(json!({
                "summary_file": summary_name, "records_file": records_name,
                "summary_sha256": format!("{:x}", Sha256::digest(&summary_bytes)),
                "frozen_records_sha256": format!("{:x}", Sha256::digest(&bytes)),
                "completed_records_sha256": format!("{:x}", Sha256::digest(std::fs::read(out.join(&records_name))?)),
                "records": count, "failures": failures,
                "metadata": summary,
            }));
        }
    }
    let manifest = json!({
        "schema_version": 1, "captured_at": chrono::Utc::now().to_rfc3339(),
        "artifact_role": "frozen_output_completion_no_retrieval",
        "transformation": "Only add absent failure classifications: empty gold, abstained, hit, partial_hit, miss; LongMemEval classes unchanged. Ordered hits, scores and metrics unchanged.",
        "unrecoverable_historical_fields": ["native numeric channel scores", "logical/physical DB bytes", "CPU model, RAM and full historical OS version"],
        "entries": entries,
    });
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out.join("frozen-artifact-completion.json"))?;
    serde_json::to_writer_pretty(&mut file, &manifest)?;
    file.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frozen_outcomes_use_existing_ids_and_metrics_only() {
        assert_eq!(
            failure(
                &json!({"evidence_sessions": ["s1", "s2"], "ranked": [{"session_id": "s1"}]}),
                "locomo10"
            )
            .unwrap(),
            "partial_hit"
        );
        assert_eq!(
            failure(&json!({"targets": [], "hit": false}), "membench").unwrap(),
            "unscorable_empty_gold"
        );
        assert_eq!(
            failure(
                &json!({"evidence_count": 2, "found": 0, "abstained": "BelowThreshold"}),
                "convomem"
            )
            .unwrap(),
            "abstained"
        );
        assert!(failure(&json!({}), "convomem").is_err());
    }
}
