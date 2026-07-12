//! Minimal embedder: grant a tool effect, answer pending calls in a loop.
//!
//! Run: cargo run --example host_effect

use kryhta::{HostValue, Result, RunOutcome, Runtime};

fn main() -> Result<()> {
    let mut rt = Runtime::new();
    rt.grant("AskHuman");

    let mut outcome = rt.eval_hosted(
        "let answer = perform AskHuman!(\"Approve the deploy?\");\n\
         perform Print!(answer);\n\
         answer",
    )?;

    while let RunOutcome::Pending(calls) = outcome {
        for call in calls {
            println!("host got effect {}({:?})", call.effect, call.args);
            rt.resume_with(call.id, HostValue::Str("approved".to_string()))?;
        }
        outcome = rt.run_hosted_continue()?;
    }

    Ok(())
}
