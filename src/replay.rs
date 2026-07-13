//! The deterministic replay log: an append-only record of every host answer
//! in application order, so a run can be re-executed from source + log alone.
//! See docs/superpowers/specs/2026-07-13-replay-log-design.md.

use crate::error::{JSError, Result};
use crate::host::HostValue;
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
}
