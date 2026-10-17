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
///
/// All twelve inputs are parsed, classified and count-checked in memory before
/// any output is created, so a malformed input leaves no partial artifact under
/// the official completed name and a corrected rerun is not blocked.
pub fn complete_frozen(runs: &Path, out: &Path) -> Result<()> {
    let mut entries = Vec::new();
    let mut outputs: Vec<(String, Vec<u8>)> = Vec::new();
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
            let mut completed = Vec::with_capacity(bytes.len());
            let mut count = 0usize;
            for line in text.lines() {
                let mut record: Value = serde_json::from_str(line)
                    .with_context(|| format!("{records_name} record {}", count + 1))?;
                if record.get("failure").is_none() {
                    record["failure"] = json!(failure(&record, benchmark)?);
                }
                *failures
                    .entry(record["failure"].as_str().context("failure")?.into())
                    .or_default() += 1;
                serde_json::to_writer(&mut completed, &record)?;
                completed.push(b'\n');
                count += 1;
            }
            if summary["overall"]["n"].as_u64() != Some(count as u64) {
                bail!("{records_name} count differs from summary");
            }
            entries.push(json!({
                "summary_file": summary_name, "records_file": records_name,
                "summary_sha256": format!("{:x}", Sha256::digest(&summary_bytes)),
                "frozen_records_sha256": format!("{:x}", Sha256::digest(&bytes)),
                "completed_records_sha256": format!("{:x}", Sha256::digest(&completed)),
                "records": count, "failures": failures,
                "metadata": summary,
            }));
            outputs.push((records_name, completed));
        }
    }
    let manifest = json!({
        "schema_version": 1, "captured_at": chrono::Utc::now().to_rfc3339(),
        "artifact_role": "frozen_output_completion_no_retrieval",
        "transformation": "Only add absent failure classifications: empty gold, abstained, hit, partial_hit, miss; LongMemEval classes unchanged. Ordered hits, scores and metrics unchanged.",
        "unrecoverable_historical_fields": ["native numeric channel scores", "logical/physical DB bytes", "CPU model, RAM and full historical OS version"],
        "entries": entries,
    });
    let mut manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    manifest_bytes.push(b'\n');
    outputs.push(("frozen-artifact-completion.json".into(), manifest_bytes));
    write_all_new(out, &outputs)
}

/// Refuse up front if any output already exists, then create each with
/// `create_new`; on a write failure, remove the files this call created.
fn write_all_new(out: &Path, outputs: &[(String, Vec<u8>)]) -> Result<()> {
    std::fs::create_dir_all(out)?;
    if let Some((name, _)) = outputs.iter().find(|(name, _)| out.join(name).exists()) {
        bail!(
            "{} already exists; refusing to overwrite frozen evidence",
            out.join(name).display()
        );
    }
    let mut created = Vec::with_capacity(outputs.len());
    let result = outputs.iter().try_for_each(|(name, bytes)| -> Result<()> {
        let path = out.join(name);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| format!("create {}", path.display()))?;
        created.push(path);
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    });
    if result.is_err() {
        for path in &created {
            let _ = std::fs::remove_file(path);
        }
    }
    result
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

    fn write_frozen_runs(runs: &Path, last_records: &str) {
        let stems = [
            "longmemeval",
            "locomo-top10",
            "convomem-top10",
            "membench-movie-top5",
        ];
        for stem in stems {
            for suffix in ["native", "parity", "parity-tok256"] {
                let tag = format!("{stem}-{suffix}");
                let summary =
                    json!({"complete": true, "benchmark": "membench", "overall": {"n": 1}});
                std::fs::write(
                    runs.join(format!("{tag}.summary.json")),
                    summary.to_string(),
                )
                .unwrap();
                let records = if tag == "membench-movie-top5-parity-tok256" {
                    last_records
                } else {
                    "{\"targets\": [\"t\"], \"hit\": true}\n"
                };
                std::fs::write(runs.join(format!("{tag}.jsonl")), records).unwrap();
            }
        }
    }

    #[test]
    fn invalid_input_creates_no_outputs_and_rerun_succeeds() {
        let runs = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        write_frozen_runs(runs.path(), "not json\n");
        assert!(complete_frozen(runs.path(), out.path()).is_err());
        assert_eq!(std::fs::read_dir(out.path()).unwrap().count(), 0);

        write_frozen_runs(runs.path(), "{\"targets\": [], \"hit\": false}\n");
        complete_frozen(runs.path(), out.path()).unwrap();
        assert_eq!(std::fs::read_dir(out.path()).unwrap().count(), 13);
        let completed =
            std::fs::read_to_string(out.path().join("membench-movie-top5-parity-tok256.jsonl"))
                .unwrap();
        assert!(completed.contains("unscorable_empty_gold"));

        let manifest = out.path().join("frozen-artifact-completion.json");
        let before = std::fs::read(&manifest).unwrap();
        let error = complete_frozen(runs.path(), out.path()).unwrap_err();
        assert!(error.to_string().contains("already exists"), "{error:#}");
        assert_eq!(std::fs::read(&manifest).unwrap(), before);
    }

    #[test]
    fn summary_count_mismatch_creates_no_outputs() {
        let runs = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        write_frozen_runs(
            runs.path(),
            "{\"targets\": [\"t\"], \"hit\": true}\n{\"targets\": [\"t\"], \"hit\": false}\n",
        );
        let error = complete_frozen(runs.path(), out.path()).unwrap_err();
        assert!(error.to_string().contains("count differs"), "{error:#}");
        assert_eq!(std::fs::read_dir(out.path()).unwrap().count(), 0);
    }
}
