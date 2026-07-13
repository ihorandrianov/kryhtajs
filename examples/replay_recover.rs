//! Crash recovery from the replay log: record a run, "crash" before
//! answering everything, then rebuild the run from the log alone and
//! finish it. The log file is self-contained — source, grants, and every
//! answer — so the second half needs nothing from the first.
//!
//! Run: cargo run --example replay_recover

use kryhta::{HostValue, Result, RunOutcome, Runtime};

fn main() -> Result<()> {
    let log = std::env::temp_dir().join("replay_recover.klog");
    let log = log.to_str().unwrap();

    // --- process one: record, answer one call, then "crash" ---
    let mut rt = Runtime::new();
    rt.grant("AskHuman")?;
    rt.record_to(log)?;
    let outcome = rt.eval_hosted(
        "let a = perform Fork!(() => perform AskHuman!(\"deploy?\"));\n\
         let b = perform Fork!(() => perform AskHuman!(\"notify?\"));\n\
         let ra = perform Join!(a);\n\
         let rb = perform Join!(b);\n\
         perform Print!(ra.ok, rb.ok);\n\
         [ra.ok, rb.ok]",
    )?;
    let RunOutcome::Pending(calls) = outcome else {
        panic!("expected pending host calls");
    };
    println!("host: {} calls pending, answering one, then crashing", calls.len());
    rt.resume_with(calls[0].id, HostValue::Str("yes".to_string()))?;
    drop(rt); // the process dies here

    // --- process two: recover from the log alone ---
    let (mut rt, outcome) = Runtime::resume_from_log(log)?;
    let RunOutcome::Pending(remaining) = outcome else {
        panic!("expected the unanswered call to resurface");
    };
    println!(
        "recovered: {} call still pending: {}({:?})",
        remaining.len(),
        remaining[0].effect,
        remaining[0].args
    );
    rt.resume_with(remaining[0].id, HostValue::Str("also yes".to_string()))?;
    let RunOutcome::Done(_) = rt.run_hosted_continue()? else {
        panic!("expected Done");
    };
    println!("run completed; the log now replays end-to-end deterministically");
    Ok(())
}
