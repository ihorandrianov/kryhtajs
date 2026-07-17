//! End-to-end CLI test: snapshot in one process, resume in another.

use std::process::Command;

#[test]
fn resume_continues_a_snapshotted_run() {
    let dir = std::env::temp_dir().join("kryhta_cli_test");
    std::fs::create_dir_all(&dir).unwrap();
    let snap = dir.join("cli.snap");
    let script = dir.join("job.js");
    std::fs::write(
        &script,
        format!(
            r#"
            var state = {{ count: 41 }};
            let outcome = perform Snapshot!("{}");
            state.count = state.count + 1;
            perform Print!(outcome, state.count);
            state.count
            "#,
            snap.to_str().unwrap()
        ),
    )
    .unwrap();

    let bin = env!("CARGO_BIN_EXE_kryhta");

    let run1 = Command::new(bin).arg(&script).output().unwrap();
    assert!(run1.status.success());
    assert!(String::from_utf8_lossy(&run1.stdout).contains("saved 42"));

    let run2 = Command::new(bin)
        .arg("--resume")
        .arg(&snap)
        .output()
        .unwrap();
    assert!(run2.status.success());
    assert!(String::from_utf8_lossy(&run2.stdout).contains("restored 42"));
}

#[test]
fn resume_rejects_garbage_files() {
    let dir = std::env::temp_dir().join("kryhta_cli_test");
    std::fs::create_dir_all(&dir).unwrap();
    let bad = dir.join("garbage.snap");
    std::fs::write(&bad, b"not a snapshot").unwrap();

    let bin = env!("CARGO_BIN_EXE_kryhta");
    let out = Command::new(bin)
        .arg("--resume")
        .arg(&bad)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("snapshot"));
}

#[test]
fn record_then_replay_reproduces_the_run() {
    let dir = std::env::temp_dir().join("kryhta_cli_test");
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("run.klog");
    let script = dir.join("pure.js");
    std::fs::write(&script, "perform Print!(\"hi\"); 40 + 2").unwrap();

    let bin = env!("CARGO_BIN_EXE_kryhta");

    let rec = Command::new(bin)
        .arg("--record")
        .arg(&log)
        .arg(&script)
        .output()
        .unwrap();
    assert!(rec.status.success(), "{}", String::from_utf8_lossy(&rec.stderr));
    assert!(String::from_utf8_lossy(&rec.stdout).contains("42"));

    // self-contained: no script argument, source comes from the log
    let rep = Command::new(bin).arg("--replay").arg(&log).output().unwrap();
    assert!(rep.status.success(), "{}", String::from_utf8_lossy(&rep.stderr));
    let out = String::from_utf8_lossy(&rep.stdout);
    assert!(out.contains("hi"), "Print re-runs live during replay: {out}");
    assert!(out.contains("42"));
}

#[test]
fn replay_rejects_garbage_logs() {
    let dir = std::env::temp_dir().join("kryhta_cli_test");
    std::fs::create_dir_all(&dir).unwrap();
    let bad = dir.join("garbage.klog");
    std::fs::write(&bad, b"not a log").unwrap();

    let bin = env!("CARGO_BIN_EXE_kryhta");
    let out = Command::new(bin).arg("--replay").arg(&bad).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("replay log"));
}

#[test]
fn fuel_flag_works_in_any_position() {
    let dir = std::env::temp_dir().join("kryhta_cli_fuel");
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("bounded.js");
    std::fs::write(&script, "let x = 0; while (x < 500) { x = x + 1; } x").unwrap();

    let bin = env!("CARGO_BIN_EXE_kryhta");

    // Flag AFTER the script path must still be honored.
    let out = Command::new(bin)
        .arg(&script)
        .arg("--fuel")
        .arg("100")
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "trailing --fuel was ignored: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("out of fuel after"));

    // Giving the flag twice is an error, not a silent override.
    let out = Command::new(bin)
        .arg("--fuel")
        .arg("100")
        .arg(&script)
        .arg("--fuel")
        .arg("200")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("more than once"));
}
