//! HIDDEN acceptance test for kanban #83 run 2 (offline pinned-model cache check).
#![cfg(feature = "local")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_embed::{cached_pin_status, PinCacheStatus};
use rb_embed::ALL_MINILM_L6_V2 as PIN;
use std::path::{Path, PathBuf};

fn snapshot(root: &Path) -> PathBuf {
    root.join("models--Qdrant--all-MiniLM-L6-v2-onnx")
        .join("snapshots")
        .join(PIN.revision)
}

fn listing(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() && !p.is_symlink() {
                stack.push(p.clone());
            }
            out.push(p);
        }
    }
    out.sort();
    out
}

#[test]
fn empty_cache_reports_every_file_missing_and_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let before = listing(dir.path());
    let status = cached_pin_status(dir.path(), &PIN).unwrap();
    assert_eq!(
        status,
        PinCacheStatus::Missing(vec![
            "model.onnx",
            "tokenizer.json",
            "config.json",
            "special_tokens_map.json",
            "tokenizer_config.json"
        ])
    );
    assert_eq!(listing(dir.path()), before, "the check must never create or download");
}

#[test]
fn a_wrong_file_is_corrupt_even_when_others_are_missing() {
    let dir = tempfile::tempdir().unwrap();
    let snap = snapshot(dir.path());
    std::fs::create_dir_all(&snap).unwrap();
    std::fs::write(snap.join("tokenizer_config.json"), b"{\"model_max_length\": 256}").unwrap();
    let status = cached_pin_status(dir.path(), &PIN).unwrap();
    assert_eq!(status, PinCacheStatus::Corrupt { file: "tokenizer_config.json" });
}

#[test]
fn symlinked_snapshot_files_are_followed_like_hf_hub_writes_them() {
    let dir = tempfile::tempdir().unwrap();
    let snap = snapshot(dir.path());
    let blobs = dir.path().join("models--Qdrant--all-MiniLM-L6-v2-onnx").join("blobs");
    std::fs::create_dir_all(&snap).unwrap();
    std::fs::create_dir_all(&blobs).unwrap();
    std::fs::write(blobs.join("bad"), b"not the config").unwrap();
    std::os::unix::fs::symlink("../../blobs/bad", snap.join("config.json")).unwrap();
    let status = cached_pin_status(dir.path(), &PIN).unwrap();
    assert_eq!(status, PinCacheStatus::Corrupt { file: "config.json" });
}

#[test]
fn correct_small_files_leave_only_the_missing_onnx() {
    // Real pinned JSON files downloaded for the oracle (PLANC_PIN_JSON_DIR).
    let src = std::env::var("PLANC_PIN_JSON_DIR")
        .expect("set PLANC_PIN_JSON_DIR to the pinned JSON files; this oracle never skips");
    let dir = tempfile::tempdir().unwrap();
    let snap = snapshot(dir.path());
    std::fs::create_dir_all(&snap).unwrap();
    for f in ["tokenizer.json", "config.json", "special_tokens_map.json", "tokenizer_config.json"] {
        std::fs::copy(Path::new(&src).join(f), snap.join(f)).unwrap();
    }
    let status = cached_pin_status(dir.path(), &PIN).unwrap();
    assert_eq!(status, PinCacheStatus::Missing(vec!["model.onnx"]));
}

#[test]
fn a_real_populated_cache_is_complete() {
    let root = std::env::var("PLANC_REAL_CACHE")
        .expect("set PLANC_REAL_CACHE to a populated model cache; this oracle never skips");
    let status = cached_pin_status(Path::new(&root), &PIN).unwrap();
    assert_eq!(status, PinCacheStatus::Complete);
}
