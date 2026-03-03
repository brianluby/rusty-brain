//! Performance benchmark tests for CLI response time.

mod common;

use std::time::Instant;

use common::{TestObs, run_cli, setup_test_mind};
use types::ObservationType;

fn make_observations(count: usize) -> Vec<TestObs> {
    (0..count)
        .map(|i| TestObs {
            obs_type: ObservationType::Discovery,
            tool_name: "Read".into(),
            summary: format!("benchmark observation number {i}"),
            content: Some(format!(
                "Content for observation {i} with some additional text for realistic sizing"
            )),
        })
        .collect()
}

#[test]
#[ignore]
fn bench_find_100_observations() {
    let observations = make_observations(100);
    let (_dir, path) = setup_test_mind(&observations);

    let start = Instant::now();
    let (status, _stdout, _stderr) = run_cli(&path, &["find", "benchmark", "--json"]);
    let elapsed = start.elapsed();

    assert!(status.success());
    eprintln!("find (100 obs): {elapsed:?}");
    assert!(
        elapsed.as_millis() < 2000,
        "find should complete in <2s for 100 obs, took {elapsed:?}"
    );
}

#[test]
#[ignore]
fn bench_stats_100_observations() {
    let observations = make_observations(100);
    let (_dir, path) = setup_test_mind(&observations);

    let start = Instant::now();
    let (status, _stdout, _stderr) = run_cli(&path, &["stats", "--json"]);
    let elapsed = start.elapsed();

    assert!(status.success());
    eprintln!("stats (100 obs): {elapsed:?}");
    assert!(
        elapsed.as_millis() < 2000,
        "stats should complete in <2s for 100 obs, took {elapsed:?}"
    );
}

#[test]
#[ignore]
fn bench_timeline_100_observations() {
    let observations = make_observations(100);
    let (_dir, path) = setup_test_mind(&observations);

    let start = Instant::now();
    let (status, _stdout, _stderr) = run_cli(&path, &["timeline", "--json"]);
    let elapsed = start.elapsed();

    assert!(status.success());
    eprintln!("timeline (100 obs): {elapsed:?}");
    assert!(
        elapsed.as_millis() < 2000,
        "timeline should complete in <2s for 100 obs, took {elapsed:?}"
    );
}
