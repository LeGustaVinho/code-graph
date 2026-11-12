use std::sync::mpsc;
use std::time::Duration;

#[test]
fn writer_lock_serializes_other_connections_and_releases_on_drop() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let first = codegraph_graph::lock_writer(&root).unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (acquired_tx, acquired_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        ready_tx.send(()).unwrap();
        let second = codegraph_graph::lock_writer(&root).unwrap();
        acquired_tx.send(()).unwrap();
        drop(second);
    });
    ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(acquired_rx.recv_timeout(Duration::from_millis(150)).is_err(), "second writer acquired the lock concurrently");
    drop(first);
    acquired_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    worker.join().unwrap();
}
