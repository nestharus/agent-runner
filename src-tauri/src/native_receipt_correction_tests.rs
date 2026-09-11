//! Opt-in residual decision probes. Failures are outstanding requirements, not
//! waived behavior. All filesystem and worker resources are private and bounded.
use super::*;
use std::sync::mpsc;

#[test]
#[ignore = "R2 unresolved: bounded shutdown probe, no production paths"]
fn age355_correction_drop_does_not_wait_for_unrelated_inspection() {
    let root = tempfile::tempdir().unwrap();
    let (entered, started) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let (finished, done) = mpsc::channel();
    let private_root = root.path().to_path_buf();
    let guard = start_receipt_polling_with(
        move || {
            // Model a synchronous private filesystem read that cannot observe
            // the stop flag. Unlike a real wedged filesystem, it has a timeout.
            std::fs::write(private_root.join("inspection-started"), b"private")
                .map_err(|e| e.to_string())?;
            entered.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(())
        },
        Duration::from_secs(60),
    )
    .unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    let dropper = std::thread::spawn(move || {
        drop(guard);
        finished.send(()).unwrap();
    });
    let returned_without_io = done.recv_timeout(Duration::from_millis(250)).is_ok();
    // Always release/join before assertion and before deleting the fixture.
    release.send(()).unwrap();
    dropper.join().unwrap();
    assert!(
        returned_without_io,
        "scope exit waited for unrelated inspection IO"
    );
}

#[test]
fn age355_stop_observed_after_io_skips_the_next_interval() {
    let (entered, started) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let (finished, done) = mpsc::channel();
    let mut guard = start_receipt_polling_with(
        move || {
            entered.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(())
        },
        Duration::from_secs(2),
    )
    .unwrap();
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    guard.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    // No unpark token: model IO consuming the token sent by Drop. The stop
    // flag itself must prevent the post-IO park, not rely on a retained token.
    let worker = guard.worker.take().unwrap();
    let joiner = std::thread::spawn(move || {
        worker.join().unwrap();
        finished.send(()).unwrap();
    });
    release.send(()).unwrap();
    let skipped_interval = done.recv_timeout(Duration::from_secs(1)).is_ok();
    joiner.join().unwrap();
    assert!(skipped_interval, "worker parked again after observing stop");
}
