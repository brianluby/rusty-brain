//! HIDDEN acceptance test for kanban #83 run 3 (socket dir hardening).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rb_daemon::prepare_socket_dir;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Barrier};

// Not covered here: refusing a directory owned by another user needs a second
// uid (root to chown), so that clause of the spec is judged by review only.

fn mode(p: &std::path::Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn missing_nested_dir_is_created_private() {
    let tmp = tempfile::tempdir().unwrap();
    let sock = tmp.path().join("a/b/run/daemon.sock");
    prepare_socket_dir(&sock).unwrap();
    assert_eq!(mode(&tmp.path().join("a/b/run")), 0o700);
}

#[test]
fn existing_private_dir_is_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    let run = tmp.path().join("run");
    std::fs::create_dir(&run).unwrap();
    std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700)).unwrap();
    prepare_socket_dir(&run.join("d.sock")).unwrap();
}

#[test]
fn existing_open_dir_is_refused_not_repaired() {
    let tmp = tempfile::tempdir().unwrap();
    let run = tmp.path().join("run");
    std::fs::create_dir(&run).unwrap();
    std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(prepare_socket_dir(&run.join("d.sock")).is_err());
    assert_eq!(mode(&run), 0o755, "must not silently chmod an existing dir");
}

#[test]
fn symlinked_socket_dir_is_refused_even_if_target_is_private() {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("real");
    std::fs::create_dir(&real).unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o700)).unwrap();
    let link = tmp.path().join("run");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let err = prepare_socket_dir(&link.join("d.sock")).unwrap_err();
    assert!(err.to_string().to_lowercase().contains("symlink"), "{err}");
}

#[test]
fn dangling_symlink_is_refused_and_not_created_through() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("nowhere");
    let link = tmp.path().join("run");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let err = prepare_socket_dir(&link.join("d.sock")).unwrap_err();
    assert!(err.to_string().to_lowercase().contains("symlink"), "{err}");
    assert!(!target.exists(), "must not create the symlink target");
}

#[test]
fn file_in_place_of_dir_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let run = tmp.path().join("run");
    std::fs::write(&run, b"x").unwrap();
    assert!(prepare_socket_dir(&run.join("d.sock")).is_err());
}

#[test]
fn concurrent_first_starts_all_succeed() {
    for round in 0..20 {
        let tmp = Arc::new(tempfile::tempdir().unwrap());
        let sock = tmp.path().join(format!("r{round}/run/d.sock"));
        // Release all 16 together so the creation race is actually exercised.
        let start = Arc::new(Barrier::new(16));
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let sock = sock.clone();
                let start = Arc::clone(&start);
                std::thread::spawn(move || {
                    start.wait();
                    prepare_socket_dir(&sock)
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap().unwrap();
        }
        assert_eq!(mode(sock.parent().unwrap()), 0o700);
    }
}
