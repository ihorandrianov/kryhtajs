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
