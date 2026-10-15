//! Portable baseline/current SQL-channel comparison. This file intentionally
//! uses only pre-#76 public APIs, so copy it unchanged into a baseline worktree.
//! Run release, one thread, same host/toolchain; compare paired repeated runs.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use rb_store::{SqliteStore, Store};
use rb_types::{MemoryNote, MemoryType, Namespace};
use std::time::Instant;

#[test]
#[ignore = "manual bounded latency measurement; not a timing assertion"]
fn unfiltered_recall_channels_latency() {
    const DIM: usize = 64;
    const SAMPLES: usize = 100;
    let ns = Namespace::Project("latency-fixture".into());
    for size in [512usize, 4096] {
        let store = SqliteStore::open_in_memory(DIM).unwrap();
        for i in 0..size {
            let mut note = MemoryNote::new(
                ns.clone(),
                format!("worker deployment retry budget evidence note {i}"),
                MemoryType::Insight,
                7,
            );
            note.tags = vec![if i % 16 == 0 { "selected" } else { "other" }.into()];
            let vector: Vec<f32> = (0..DIM)
                .map(|j| (((i * 17 + j * 13) % 101) + 1) as f32 / 101.0)
                .collect();
            store.insert_memory(&note, Some(&vector)).unwrap();
        }
        let query = vec![0.5f32; DIM];
        for _ in 0..20 {
            assert_eq!(
                store
                    .keyword_search(&ns, "worker deployment", 40)
                    .unwrap()
                    .len(),
                40
            );
            assert_eq!(store.vector_search(&ns, &query, 40).unwrap().len(), 40);
        }
        let mut keyword_us = Vec::with_capacity(SAMPLES);
        let mut vector_us = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let start = Instant::now();
            std::hint::black_box(store.keyword_search(&ns, "worker deployment", 40).unwrap());
            keyword_us.push(start.elapsed().as_nanos() as f64 / 1000.0);
            let start = Instant::now();
            std::hint::black_box(store.vector_search(&ns, &query, 40).unwrap());
            vector_us.push(start.elapsed().as_nanos() as f64 / 1000.0);
        }
        for (channel, mut samples) in [("keyword", keyword_us), ("vector", vector_us)] {
            samples.sort_by(f64::total_cmp);
            println!("rows={size} dim={DIM} limit=40 warmups=20 samples={SAMPLES} channel={channel} median_us={:.3} p95_us={:.3}", samples[SAMPLES / 2], samples[SAMPLES * 95 / 100]);
        }
    }
}
