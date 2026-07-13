//! The deterministic replay log: an append-only record of every host answer
//! in application order, so a run can be re-executed from source + log alone.
//! See docs/superpowers/specs/2026-07-13-replay-log-design.md.

use crate::error::{JSError, Result};
use crate::host::{HostValue, RunOutcome};
use crate::parser::Parser;
use crate::runtime::{FiberId, FiberStatus};
use crate::snapshot::{read_host_value, write_host_value, ByteReader, ByteWriter};

pub(crate) const LOG_MAGIC: &[u8; 4] = b"KRLG";
pub(crate) const LOG_VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LogHeader {
    pub source: String,
    pub grants: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum LogEvent {
    /// One `resume_with`: the answer that was applied, plus the call it
    /// answered (effect + args) so replay can verify, and audits can read.
    HostAnswer {
        call_id: u32,
        effect: String,
        args: Vec<HostValue>,
        answer: HostValue,
    },
    /// One `run_hosted_continue`: scheduler re-entry. The order of these
    /// relative to answers is what makes fiber scheduling reproducible.
    Continue,
    /// Run completed. The result is recorded when it can cross the host
    /// boundary (plain data); a code value records as None.
    Done { result: Option<HostValue> },
}

#[derive(Debug)]
pub(crate) struct ParsedLog {
    pub header: LogHeader,
    pub events: Vec<LogEvent>,
    /// Byte offset of the end of the last complete record — where appending
    /// resumes, dropping any torn tail a crash left behind.
    pub good_len: u64,
}

pub(crate) fn encode_header(h: &LogHeader) -> Vec<u8> {
    let mut w = ByteWriter::new();
    for &b in LOG_MAGIC {
        w.u8(b);
    }
    w.u8(LOG_VERSION);
    w.str_(&h.source);
    w.u32(h.grants.len() as u32);
    for g in &h.grants {
        w.str_(g);
    }
    w.finish()
}

pub(crate) fn encode_event(e: &LogEvent) -> Vec<u8> {
    let mut w = ByteWriter::new();
    match e {
        LogEvent::HostAnswer {
            call_id,
            effect,
            args,
            answer,
        } => {
            w.u8(1);
            w.u32(*call_id);
            w.str_(effect);
            w.u32(args.len() as u32);
            for a in args {
                write_host_value(&mut w, a);
            }
            write_host_value(&mut w, answer);
        }
        LogEvent::Continue => w.u8(2),
        LogEvent::Done { result } => {
            w.u8(3);
            match result {
                Some(v) => {
                    w.bool_(true);
                    write_host_value(&mut w, v);
                }
                None => w.bool_(false),
            }
        }
    }
    let payload = w.finish();
    let mut out = Vec::with_capacity(payload.len() + 4);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&payload);
    out
}

fn decode_event(payload: &[u8]) -> Result<LogEvent> {
    let mut r = ByteReader::new(payload);
    let event = match r.u8()? {
        1 => {
            let call_id = r.u32()?;
            let effect = r.str_()?;
            let n = r.u32()? as usize;
            let mut args = Vec::with_capacity(n.min(1 << 16));
            for _ in 0..n {
                args.push(read_host_value(&mut r)?);
            }
            let answer = read_host_value(&mut r)?;
            LogEvent::HostAnswer {
                call_id,
                effect,
                args,
                answer,
            }
        }
        2 => LogEvent::Continue,
        3 => {
            let result = if r.bool_()? {
                Some(read_host_value(&mut r)?)
            } else {
                None
            };
            LogEvent::Done { result }
        }
        tag => {
            return Err(JSError::Message(format!(
                "replay log: bad event tag {tag}"
            )));
        }
    };
    if !r.is_at_end() {
        return Err(JSError::Message(
            "replay log: trailing bytes inside an event record".to_string(),
        ));
    }
    Ok(event)
}

pub(crate) fn parse_log(bytes: &[u8]) -> Result<ParsedLog> {
    let mut r = ByteReader::new(bytes);
    let mut magic = [0u8; 4];
    for m in &mut magic {
        *m = r
            .u8()
            .map_err(|_| JSError::Message("replay log: bad magic".to_string()))?;
    }
    if &magic != LOG_MAGIC {
        return Err(JSError::Message("replay log: bad magic".to_string()));
    }
    let version = r.u8()?;
    if version != LOG_VERSION {
        return Err(JSError::Message(format!(
            "replay log: unsupported version {version} (expected {LOG_VERSION})"
        )));
    }
    let source = r.str_()?;
    let n = r.u32()? as usize;
    let mut grants = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        grants.push(r.str_()?);
    }
    let header = LogHeader { source, grants };

    // Events are length-prefixed so a crash mid-append (torn tail) is
    // distinguishable from corruption: incomplete trailing bytes end the
    // log; a complete record that fails to decode is an error.
    let mut events = Vec::new();
    let mut off = r.pos();
    loop {
        let rem = &bytes[off..];
        if rem.len() < 4 {
            break;
        }
        let len = u32::from_le_bytes(rem[..4].try_into().unwrap()) as usize;
        if rem.len() - 4 < len {
            break;
        }
        let event = decode_event(&rem[4..4 + len])?;
        if matches!(events.last(), Some(LogEvent::Done { .. })) {
            return Err(JSError::Message(
                "replay log: events after Done".to_string(),
            ));
        }
        events.push(event);
        off += 4 + len;
    }
    Ok(ParsedLog {
        header,
        events,
        good_len: off as u64,
    })
}

/// Strict equality for verification: floats compare by bit pattern, so a
/// recorded NaN matches a replayed NaN and -0.0 differs from 0.0. Derived
/// PartialEq (NaN != NaN) would produce false divergence errors.
pub(crate) fn host_value_bit_eq(a: &HostValue, b: &HostValue) -> bool {
    match (a, b) {
        (HostValue::Undefined, HostValue::Undefined) => true,
        (HostValue::Null, HostValue::Null) => true,
        (HostValue::Bool(x), HostValue::Bool(y)) => x == y,
        (HostValue::Int(x), HostValue::Int(y)) => x == y,
        (HostValue::Float(x), HostValue::Float(y)) => x.to_bits() == y.to_bits(),
        (HostValue::Str(x), HostValue::Str(y)) => x == y,
        (HostValue::Array(xs), HostValue::Array(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| host_value_bit_eq(x, y))
        }
        (HostValue::Object(xs), HostValue::Object(ys)) => {
            xs.len() == ys.len()
                && xs
                    .iter()
                    .zip(ys)
                    .all(|((kx, x), (ky, y))| kx == ky && host_value_bit_eq(x, y))
        }
        _ => false,
    }
}

fn io_err(context: &str, e: std::io::Error) -> JSError {
    JSError::Message(format!("replay log: {context}: {e}"))
}

pub(crate) struct LogWriter {
    file: std::fs::File,
    /// Once Done is on disk the run is over; guards against a host loop
    /// calling run_hosted_continue again and appending events after Done.
    pub(crate) done_written: bool,
}

impl LogWriter {
    pub(crate) fn create(path: &str, header: &LogHeader) -> Result<Self> {
        use std::io::Write;
        let mut file =
            std::fs::File::create(path).map_err(|e| io_err("cannot create log", e))?;
        file.write_all(&encode_header(header))
            .map_err(|e| io_err("cannot write header", e))?;
        file.sync_data().map_err(|e| io_err("cannot sync", e))?;
        Ok(Self {
            file,
            done_written: false,
        })
    }

    /// Reopen for appending after replay. Truncates to `good_len` first,
    /// dropping any torn tail a crash left behind.
    pub(crate) fn open_at(path: &str, good_len: u64) -> Result<Self> {
        use std::io::{Seek, SeekFrom};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|e| io_err("cannot open log", e))?;
        file.set_len(good_len)
            .map_err(|e| io_err("cannot truncate torn tail", e))?;
        file.seek(SeekFrom::End(0))
            .map_err(|e| io_err("cannot seek", e))?;
        Ok(Self {
            file,
            done_written: false,
        })
    }

    /// Write-ahead: synced to disk before the caller applies the event to
    /// the runtime, so the log never lags the state it explains.
    pub(crate) fn append(&mut self, event: &LogEvent) -> Result<()> {
        use std::io::Write;
        self.file
            .write_all(&encode_event(event))
            .map_err(|e| io_err("cannot append", e))?;
        self.file.sync_data().map_err(|e| io_err("cannot sync", e))
    }
}

pub(crate) enum LogMode {
    Off,
    /// record_to was called; the header (which needs the source text) is
    /// written when eval_hosted starts the run.
    Armed { path: String },
    Recording(LogWriter),
    Replaying,
}

use crate::runtime::Runtime;

impl Runtime {
    /// Arm recording. The run itself must start via `eval_hosted` — the
    /// header embeds the source text, which AST entry points don't have.
    pub fn record_to(&mut self, path: &str) -> Result<()> {
        if self.next_fiber_id != 0 || !self.fibers.is_empty() {
            return Err(JSError::Message(
                "record_to: recording requires a fresh runtime (one that has not run and was not snapshot-restored)"
                    .to_string(),
            ));
        }
        if !matches!(self.log, LogMode::Off) {
            return Err(JSError::Message(
                "record_to: a log is already attached to this runtime".to_string(),
            ));
        }
        self.log = LogMode::Armed {
            path: path.to_string(),
        };
        Ok(())
    }

    /// Rebuild a run from its log: re-execute the embedded source, feeding
    /// logged answers back with strict verification, then continue live —
    /// recording — on the same file. This is both crash recovery and audit:
    /// a divergence can only mean runtime nondeterminism or a tampered log,
    /// never "the script changed", because the source comes from the log.
    pub fn resume_from_log(path: &str) -> Result<(Runtime, RunOutcome)> {
        let bytes = std::fs::read(path)
            .map_err(|e| JSError::Message(format!("replay log: cannot read {path}: {e}")))?;
        let ParsedLog {
            header,
            events,
            good_len,
        } = parse_log(&bytes)?;

        let mut rt = Runtime::new();
        for name in &header.grants {
            rt.grant(name)?;
        }
        rt.log = LogMode::Replaying;

        // Mirror eval_hosted's parse-then-run, minus its mode guards.
        let parser = Parser::with_state(
            &header.source,
            rt.interpreter.strings.clone(),
            rt.ast.clone(),
        )?;
        let (arena, strings) = parser.parse_program()?;
        rt.interpreter.strings = strings;
        rt.ast = arena;
        let ast = std::mem::take(&mut rt.ast);
        let initial = rt.run_hosted_fresh(&ast);
        rt.ast = ast;
        let mut outcome = initial?;

        let mut unflushed_answers = false;
        let mut saw_done = false;
        for (i, event) in events.iter().enumerate() {
            match event {
                LogEvent::HostAnswer {
                    call_id,
                    effect,
                    args,
                    answer,
                } => {
                    rt.verify_pending_matches(*call_id, effect, args)
                        .map_err(|e| {
                            JSError::Message(format!("replay diverged at event {i}: {e}"))
                        })?;
                    rt.apply_host_answer(FiberId(*call_id), answer);
                    unflushed_answers = true;
                }
                LogEvent::Continue => {
                    outcome = rt.continue_scheduler()?;
                    unflushed_answers = false;
                }
                LogEvent::Done { result } => {
                    let RunOutcome::Done(v) = &outcome else {
                        return Err(JSError::Message(format!(
                            "replay diverged at event {i}: log records Done but the run is still pending"
                        )));
                    };
                    if let Some(expected) = result {
                        let actual = crate::host::to_host_value(&rt.interpreter, *v).ok();
                        let matches =
                            actual.as_ref().is_some_and(|a| host_value_bit_eq(a, expected));
                        if !matches {
                            return Err(JSError::Message(format!(
                                "replay diverged at event {i}: final result does not match the recorded one"
                            )));
                        }
                    }
                    saw_done = true;
                }
            }
        }

        // Replay caught up — continue live, appending to the same file.
        let mut writer = LogWriter::open_at(path, good_len)?;
        writer.done_written = saw_done;
        rt.log = LogMode::Recording(writer);

        if unflushed_answers {
            // The original host died between answering and continuing;
            // perform the continue it would have (recording it, live).
            outcome = rt.run_hosted_continue()?;
        } else {
            // Re-seal completion if the crash ate the Done record.
            outcome = rt.finish_run(Ok(outcome))?;
        }
        Ok((rt, outcome))
    }

    /// Strict divergence check for one logged answer against live state.
    fn verify_pending_matches(
        &self,
        call_id: u32,
        effect: &str,
        args: &[HostValue],
    ) -> Result<()> {
        let fiber_id = FiberId(call_id);
        let Some(fiber) = self.fibers.iter().find(|f| f.id == fiber_id) else {
            return Err(JSError::Message(format!(
                "log answers call {call_id}, but no such fiber exists"
            )));
        };
        let FiberStatus::BlockedOnHost {
            effect: actual_effect,
            args: actual_args,
        } = &fiber.status
        else {
            return Err(JSError::Message(format!(
                "log answers call {call_id}, but that fiber is not blocked on a host call"
            )));
        };
        let actual_name = self.interpreter.strings.get(*actual_effect).unwrap_or("");
        if actual_name != effect {
            return Err(JSError::Message(format!(
                "log answers effect '{effect}', but the run performed '{actual_name}'"
            )));
        }
        let args_match = actual_args.len() == args.len()
            && actual_args
                .iter()
                .zip(args)
                .all(|(a, b)| host_value_bit_eq(a, b));
        if !args_match {
            return Err(JSError::Message(format!(
                "args for effect '{effect}' do not match the log"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::HostValue;

    fn sample_answer() -> LogEvent {
        LogEvent::HostAnswer {
            call_id: 3,
            effect: "Ask".to_string(),
            args: vec![
                HostValue::Str("q".to_string()),
                HostValue::Array(vec![HostValue::Float(f64::NAN)]),
            ],
            answer: HostValue::Object(vec![("ok".to_string(), HostValue::Int(1))]),
        }
    }

    fn sample_log(events: &[LogEvent]) -> Vec<u8> {
        let header = LogHeader {
            source: "perform Ask!(\"q\")".to_string(),
            grants: vec!["Ask".to_string()],
        };
        let mut bytes = encode_header(&header);
        for e in events {
            bytes.extend_from_slice(&encode_event(e));
        }
        bytes
    }

    #[test]
    fn header_and_events_round_trip() {
        let events = [
            sample_answer(),
            LogEvent::Continue,
            LogEvent::Done {
                result: Some(HostValue::Int(7)),
            },
        ];
        let bytes = sample_log(&events);
        let parsed = parse_log(&bytes).unwrap();
        assert_eq!(parsed.header.source, "perform Ask!(\"q\")");
        assert_eq!(parsed.header.grants, vec!["Ask".to_string()]);
        assert_eq!(parsed.events.len(), 3);
        assert!(matches!(parsed.events[1], LogEvent::Continue));
        assert_eq!(parsed.good_len, bytes.len() as u64);
        // NaN survives: bit-exact, not PartialEq
        let LogEvent::HostAnswer { args, .. } = &parsed.events[0] else {
            panic!("expected HostAnswer");
        };
        let HostValue::Array(items) = &args[1] else {
            panic!("expected array arg");
        };
        let HostValue::Float(f) = items[0] else {
            panic!("expected float");
        };
        assert!(f.is_nan());
    }

    #[test]
    fn done_without_result_round_trips() {
        let bytes = sample_log(&[LogEvent::Done { result: None }]);
        let parsed = parse_log(&bytes).unwrap();
        assert!(matches!(parsed.events[0], LogEvent::Done { result: None }));
    }

    #[test]
    fn torn_tail_is_end_of_log_not_an_error() {
        let complete = sample_log(&[sample_answer()]);
        let good_len = complete.len();
        let mut torn = sample_log(&[sample_answer(), LogEvent::Continue]);
        torn.truncate(good_len + 2); // partial second record
        let parsed = parse_log(&torn).unwrap();
        assert_eq!(parsed.events.len(), 1);
        assert_eq!(parsed.good_len, good_len as u64);
    }

    #[test]
    fn bad_event_tag_is_corruption() {
        let mut bytes = sample_log(&[]);
        bytes.extend_from_slice(&1u32.to_le_bytes()); // len 1
        bytes.push(99); // bogus tag
        let err = parse_log(&bytes).unwrap_err();
        assert!(err.to_string().contains("tag"), "{err}");
    }

    #[test]
    fn events_after_done_are_corruption() {
        let bytes = sample_log(&[LogEvent::Done { result: None }, LogEvent::Continue]);
        let err = parse_log(&bytes).unwrap_err();
        assert!(err.to_string().contains("after Done"), "{err}");
    }

    #[test]
    fn bad_magic_and_version_are_errors() {
        let err = parse_log(b"NOPE").unwrap_err();
        assert!(err.to_string().contains("magic"), "{err}");
        let mut bytes = sample_log(&[]);
        bytes[4] = 200; // version byte follows the 4-byte magic
        let err = parse_log(&bytes).unwrap_err();
        assert!(err.to_string().contains("version"), "{err}");
    }

    #[test]
    fn bit_eq_is_strict_about_floats() {
        use HostValue::*;
        assert!(host_value_bit_eq(&Float(f64::NAN), &Float(f64::NAN)));
        assert!(!host_value_bit_eq(&Float(0.0), &Float(-0.0)));
        assert!(!host_value_bit_eq(&Int(1), &Float(1.0)));
        assert!(host_value_bit_eq(
            &Object(vec![("a".to_string(), Null)]),
            &Object(vec![("a".to_string(), Null)])
        ));
        assert!(!host_value_bit_eq(
            &Object(vec![("a".to_string(), Null)]),
            &Object(vec![("b".to_string(), Null)])
        ));
    }

    use crate::runtime::Runtime;

    fn temp_path(name: &str) -> String {
        let dir = std::env::temp_dir().join("kryhta_replay_test");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name).to_str().unwrap().to_string()
    }

    #[test]
    fn record_to_requires_a_fresh_runtime() {
        let mut rt = Runtime::new();
        rt.eval("1 + 1").unwrap();
        let err = rt.record_to(&temp_path("used.klog")).unwrap_err();
        assert!(err.to_string().contains("fresh runtime"), "{err}");
    }

    #[test]
    fn record_to_rejects_a_snapshot_restored_runtime() {
        let mut rt = Runtime::new();
        rt.grant("Ask").unwrap();
        rt.eval_hosted("perform Ask!(1)").unwrap();
        let ready = rt.ready_queue.clone();
        let bytes = crate::snapshot::write_runtime(&rt, &ready);
        let mut rt2 = Runtime::from_snapshot(&bytes).unwrap();
        let err = rt2.record_to(&temp_path("restored.klog")).unwrap_err();
        assert!(err.to_string().contains("fresh runtime"), "{err}");
    }

    #[test]
    fn recorded_eval_writes_the_header() {
        let path = temp_path("header.klog");
        let mut rt = Runtime::new();
        rt.grant("Zeta").unwrap();
        rt.grant("Ask").unwrap();
        rt.record_to(&path).unwrap();
        rt.eval_hosted("perform Ask!(\"q\")").unwrap();
        let parsed = parse_log(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(parsed.header.source, "perform Ask!(\"q\")");
        // sorted for byte-stable headers regardless of grant order
        assert_eq!(parsed.header.grants, vec!["Ask".to_string(), "Zeta".to_string()]);
    }

    #[test]
    fn grants_freeze_once_a_recorded_run_starts() {
        let path = temp_path("frozen.klog");
        let mut rt = Runtime::new();
        rt.grant("Ask").unwrap();
        rt.record_to(&path).unwrap();
        rt.eval_hosted("perform Ask!(1)").unwrap();
        let err = rt.grant("Late").unwrap_err();
        assert!(err.to_string().contains("frozen"), "{err}");
    }

    #[test]
    fn armed_runtime_rejects_ast_entry_points() {
        let path = temp_path("armed.klog");
        let mut rt = Runtime::new();
        rt.record_to(&path).unwrap();
        let err = rt.eval("1 + 1").unwrap_err();
        assert!(err.to_string().contains("eval_hosted"), "{err}");
    }

    use crate::host::{HostValue as HV, RunOutcome};

    #[test]
    fn recording_captures_answers_continues_and_done_in_order() {
        let path = temp_path("session.klog");
        let mut rt = Runtime::new();
        rt.grant("Ask").unwrap();
        rt.record_to(&path).unwrap();
        let RunOutcome::Pending(calls) = rt
            .eval_hosted(
                "let a = perform Fork!(() => perform Ask!(\"first\"));\n\
                 let b = perform Fork!(() => perform Ask!(\"second\"));\n\
                 let ra = perform Join!(a);\n\
                 let rb = perform Join!(b);\n\
                 [ra.ok, rb.ok]",
            )
            .unwrap()
        else {
            panic!("expected Pending");
        };
        assert_eq!(calls.len(), 2);
        let first = calls
            .iter()
            .find(|c| c.args == vec![HV::Str("first".into())])
            .unwrap();
        let second = calls
            .iter()
            .find(|c| c.args == vec![HV::Str("second".into())])
            .unwrap();

        // answer out of order, in two batches with a continue between
        rt.resume_with(second.id, HV::Int(2)).unwrap();
        let RunOutcome::Pending(_) = rt.run_hosted_continue().unwrap() else {
            panic!("first still pending");
        };
        rt.resume_with(first.id, HV::Int(1)).unwrap();
        let RunOutcome::Done(_) = rt.run_hosted_continue().unwrap() else {
            panic!("expected Done");
        };

        let parsed = parse_log(&std::fs::read(&path).unwrap()).unwrap();
        let kinds: Vec<u8> = parsed
            .events
            .iter()
            .map(|e| match e {
                LogEvent::HostAnswer { .. } => 1,
                LogEvent::Continue => 2,
                LogEvent::Done { .. } => 3,
            })
            .collect();
        assert_eq!(kinds, vec![1, 2, 1, 2, 3]);
        let LogEvent::HostAnswer { effect, args, answer, .. } = &parsed.events[0] else {
            panic!();
        };
        assert_eq!(effect, "Ask");
        assert_eq!(args, &vec![HV::Str("second".into())]);
        assert_eq!(answer, &HV::Int(2));
        let LogEvent::Done { result } = &parsed.events[4] else {
            panic!();
        };
        assert_eq!(
            result,
            &Some(HV::Array(vec![HV::Int(1), HV::Int(2)]))
        );
    }

    #[test]
    fn unconvertible_final_result_records_done_without_a_value() {
        let path = temp_path("fnresult.klog");
        let mut rt = Runtime::new();
        rt.record_to(&path).unwrap();
        rt.eval_hosted("(x) => x").unwrap();
        let parsed = parse_log(&std::fs::read(&path).unwrap()).unwrap();
        assert!(matches!(
            parsed.events.as_slice(),
            [LogEvent::Done { result: None }]
        ));
    }

    #[test]
    fn recording_io_failure_surfaces_before_the_run_starts() {
        let mut rt = Runtime::new();
        rt.record_to("/nonexistent_dir_kryhta/x.klog").unwrap();
        let err = rt.eval_hosted("1 + 1").unwrap_err();
        assert!(err.to_string().contains("cannot create log"), "{err}");
    }

    #[test]
    fn flagship_record_crash_recover_then_full_replay() {
        let path = temp_path("flagship.klog");
        let script = "let a = perform Fork!(() => perform Ask!(\"one\"));\n\
                      let b = perform Fork!(() => perform Ask!(\"two\"));\n\
                      let ra = perform Join!(a);\n\
                      let rb = perform Join!(b);\n\
                      [ra.ok, rb.ok]";

        // original run: answer only "one", then the process "dies"
        let mut rt = Runtime::new();
        rt.grant("Ask").unwrap();
        rt.record_to(&path).unwrap();
        let RunOutcome::Pending(calls) = rt.eval_hosted(script).unwrap() else {
            panic!()
        };
        let one = calls
            .iter()
            .find(|c| c.args == vec![HV::Str("one".into())])
            .unwrap();
        rt.resume_with(one.id, HV::Int(10)).unwrap();
        let expected_two_id = calls
            .iter()
            .find(|c| c.args == vec![HV::Str("two".into())])
            .unwrap()
            .id;
        drop(rt);

        // recovery: replay catches up and surfaces the unanswered call
        let (mut rt2, outcome) = Runtime::resume_from_log(&path).unwrap();
        let RunOutcome::Pending(remaining) = outcome else {
            panic!("expected the unanswered call to resurface");
        };
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, expected_two_id);
        assert_eq!(remaining[0].effect, "Ask");
        assert_eq!(remaining[0].args, vec![HV::Str("two".into())]);

        rt2.resume_with(remaining[0].id, HV::Int(20)).unwrap();
        let RunOutcome::Done(v) = rt2.run_hosted_continue().unwrap() else {
            panic!()
        };
        let hv = crate::host::to_host_value(&rt2.interpreter, v).unwrap();
        assert_eq!(hv, HV::Array(vec![HV::Int(10), HV::Int(20)]));
        drop(rt2);

        // the healed log now replays end-to-end without any live answers
        let (_rt3, outcome) = Runtime::resume_from_log(&path).unwrap();
        let RunOutcome::Done(_) = outcome else {
            panic!("complete log must replay straight to Done");
        };
        let parsed = parse_log(&std::fs::read(&path).unwrap()).unwrap();
        assert!(matches!(
            parsed.events.last(),
            Some(LogEvent::Done { result: Some(_) })
        ));
    }

    #[test]
    fn replay_is_self_contained_no_script_needed() {
        let path = temp_path("selfcontained.klog");
        let mut rt = Runtime::new();
        rt.record_to(&path).unwrap();
        rt.eval_hosted("40 + 2").unwrap();
        drop(rt);
        let (_rt, outcome) = Runtime::resume_from_log(&path).unwrap();
        let RunOutcome::Done(v) = outcome else { panic!() };
        assert_eq!(v, crate::value::JSValue::Int(42));
    }

    #[test]
    fn mid_batch_crash_auto_continues_and_heals_the_log() {
        let path = temp_path("midbatch.klog");
        let mut rt = Runtime::new();
        rt.grant("Ask").unwrap();
        rt.record_to(&path).unwrap();
        let RunOutcome::Pending(calls) = rt.eval_hosted("perform Ask!(\"q\")").unwrap() else {
            panic!()
        };
        // answer but never call run_hosted_continue — died mid-batch
        rt.resume_with(calls[0].id, HV::Int(5)).unwrap();
        drop(rt);

        let (_rt2, outcome) = Runtime::resume_from_log(&path).unwrap();
        let RunOutcome::Done(v) = outcome else {
            panic!("auto-continue must finish the run");
        };
        assert_eq!(v, crate::value::JSValue::Int(5));
        // the auto-continue and completion were recorded
        let parsed = parse_log(&std::fs::read(&path).unwrap()).unwrap();
        assert!(matches!(parsed.events.last(), Some(LogEvent::Done { .. })));
    }

    /// A log whose single event is forged; the genuine run performs
    /// Ask!("q") and any mismatch with the forged record must diverge.
    fn forged_log(path: &str, event: LogEvent) -> String {
        let p = temp_path(path);
        let header = LogHeader {
            source: "perform Ask!(\"q\")".to_string(),
            grants: vec!["Ask".to_string()],
        };
        let mut bytes = encode_header(&header);
        bytes.extend_from_slice(&encode_event(&event));
        std::fs::write(&p, &bytes).unwrap();
        p
    }

    #[test]
    fn tampered_args_diverge() {
        let p = forged_log(
            "tamper_args.klog",
            LogEvent::HostAnswer {
                call_id: 0,
                effect: "Ask".to_string(),
                args: vec![HV::Str("FORGED".into())],
                answer: HV::Int(1),
            },
        );
        let Err(err) = Runtime::resume_from_log(&p) else {
            panic!("expected error");
        };
        assert!(err.to_string().contains("diverged at event 0"), "{err}");
        assert!(err.to_string().contains("args"), "{err}");
    }

    #[test]
    fn tampered_effect_name_diverges() {
        let p = forged_log(
            "tamper_effect.klog",
            LogEvent::HostAnswer {
                call_id: 0,
                effect: "Evil".to_string(),
                args: vec![HV::Str("q".into())],
                answer: HV::Int(1),
            },
        );
        let Err(err) = Runtime::resume_from_log(&p) else {
            panic!("expected error");
        };
        assert!(err.to_string().contains("'Evil'"), "{err}");
    }

    #[test]
    fn tampered_call_id_diverges() {
        let p = forged_log(
            "tamper_id.klog",
            LogEvent::HostAnswer {
                call_id: 42,
                effect: "Ask".to_string(),
                args: vec![HV::Str("q".into())],
                answer: HV::Int(1),
            },
        );
        let Err(err) = Runtime::resume_from_log(&p) else {
            panic!("expected error");
        };
        assert!(err.to_string().contains("no such fiber"), "{err}");
    }

    #[test]
    fn premature_done_diverges() {
        let p = forged_log("early_done.klog", LogEvent::Done { result: None });
        let Err(err) = Runtime::resume_from_log(&p) else {
            panic!("expected error");
        };
        assert!(err.to_string().contains("still pending"), "{err}");
    }

    #[test]
    fn tampered_final_result_diverges() {
        let path = temp_path("tamper_done.klog");
        let mut rt = Runtime::new();
        rt.record_to(&path).unwrap();
        rt.eval_hosted("40 + 2").unwrap();
        drop(rt);
        // rewrite the log with a forged Done value
        let parsed = parse_log(&std::fs::read(&path).unwrap()).unwrap();
        let mut bytes = encode_header(&parsed.header);
        bytes.extend_from_slice(&encode_event(&LogEvent::Done {
            result: Some(HV::Int(999)),
        }));
        std::fs::write(&path, &bytes).unwrap();
        let Err(err) = Runtime::resume_from_log(&path) else {
            panic!("expected error");
        };
        assert!(err.to_string().contains("final result"), "{err}");
    }

    #[test]
    fn torn_tail_recovers_and_reseals_the_log() {
        let path = temp_path("torn.klog");
        let mut rt = Runtime::new();
        rt.grant("Ask").unwrap();
        rt.record_to(&path).unwrap();
        let RunOutcome::Pending(calls) = rt.eval_hosted("perform Ask!(\"q\")").unwrap() else {
            panic!()
        };
        rt.resume_with(calls[0].id, HV::Int(5)).unwrap();
        rt.run_hosted_continue().unwrap();
        drop(rt);

        // crash mid-append: chop bytes off the tail (into the Done record)
        let full = std::fs::read(&path).unwrap();
        std::fs::write(&path, &full[..full.len() - 3]).unwrap();

        let (_rt, outcome) = Runtime::resume_from_log(&path).unwrap();
        let RunOutcome::Done(v) = outcome else { panic!() };
        assert_eq!(v, crate::value::JSValue::Int(5));
        // the tail was truncated and Done re-sealed: the log parses complete
        let parsed = parse_log(&std::fs::read(&path).unwrap()).unwrap();
        assert!(matches!(parsed.events.last(), Some(LogEvent::Done { .. })));
    }

    #[test]
    fn nan_crossing_the_boundary_replays_without_false_divergence() {
        let path = temp_path("nan.klog");
        let script = "let x = perform Ask!(\"gimme\");\n\
                      let y = perform Ask!(x);\n\
                      y";
        let mut rt = Runtime::new();
        rt.grant("Ask").unwrap();
        rt.record_to(&path).unwrap();
        let RunOutcome::Pending(calls) = rt.eval_hosted(script).unwrap() else {
            panic!()
        };
        // answer NaN; the script passes it back as the second call's arg,
        // so the recorded args contain NaN and strict verify must not trip
        rt.resume_with(calls[0].id, HV::Float(f64::NAN)).unwrap();
        let RunOutcome::Pending(calls2) = rt.run_hosted_continue().unwrap() else {
            panic!()
        };
        rt.resume_with(calls2[0].id, HV::Int(1)).unwrap();
        rt.run_hosted_continue().unwrap();
        drop(rt);

        let (_rt, outcome) = Runtime::resume_from_log(&path).unwrap();
        assert!(matches!(outcome, RunOutcome::Done(_)));
    }
}
