//! The host boundary: self-contained values that cross between the runtime
//! and a Rust embedder in both directions. The host never touches arena ids.

use crate::cekh::CEKH;
use crate::error::{JSError, Result};
use crate::object::{Object, ObjectKind};
use crate::runtime::FiberId;
use crate::value::{JSValue, ObjId};
use std::collections::HashSet;

/// Identifies one pending host call. It is the blocked fiber's id —
/// fiber ids are monotonic and never reused, and a fiber has at most
/// one pending effect, so no separate counter exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallId(pub(crate) FiberId);

#[derive(Debug, Clone)]
pub struct PendingCall {
    pub id: CallId,
    pub effect: String,
    pub args: Vec<HostValue>,
}

#[derive(Debug, Clone)]
pub enum RunOutcome {
    Done(JSValue),
    Pending(Vec<PendingCall>),
    /// The root fuel budget ran dry. The run is paused, snapshottable,
    /// and resumable: `add_fuel` then `run_hosted_continue`.
    OutOfFuel { spent: u64 },
}

#[derive(Debug, Clone, PartialEq)]
pub enum HostValue {
    Undefined,
    Null,
    Bool(bool),
    Int(i32),
    Float(f64),
    Str(String),
    Array(Vec<HostValue>),
    Object(Vec<(String, HostValue)>),
}

/// Deep-convert a runtime value into a self-contained host value.
/// Functions, handlers, and continuations cannot cross the boundary;
/// cycles are an error rather than a hang.
pub fn to_host_value(interp: &CEKH, v: JSValue) -> Result<HostValue> {
    let mut visiting = HashSet::new();
    convert_out(interp, v, &mut visiting)
}

fn convert_out(interp: &CEKH, v: JSValue, visiting: &mut HashSet<u32>) -> Result<HostValue> {
    Ok(match v {
        JSValue::Undefined => HostValue::Undefined,
        JSValue::Null => HostValue::Null,
        JSValue::Bool(b) => HostValue::Bool(b),
        JSValue::Int(i) => HostValue::Int(i),
        JSValue::Float(f) => HostValue::Float(f),
        JSValue::String(id) => HostValue::Str(interp.strings.get(id).unwrap_or("").to_string()),
        JSValue::Array(id) | JSValue::Object(id) => {
            if !visiting.insert(id.0) {
                return Err(JSError::Message(
                    "host boundary: cyclic object cannot cross".to_string(),
                ));
            }
            let obj = interp
                .objects
                .get(id.into_arena_id())
                .ok_or(JSError::InternalError("host boundary: dangling object id"))?;
            let out = match &obj.kind {
                ObjectKind::Array(data) => {
                    let mut items = Vec::with_capacity(data.elements.len());
                    for el in &data.elements {
                        items.push(convert_out(interp, *el, visiting)?);
                    }
                    HostValue::Array(items)
                }
                ObjectKind::Ordinary => {
                    let mut pairs = Vec::with_capacity(obj.properties.len());
                    for (key, prop) in &obj.properties {
                        let name = interp.strings.get(*key).unwrap_or("").to_string();
                        pairs.push((name, convert_out(interp, prop.value, visiting)?));
                    }
                    // HashMap iteration order is nondeterministic; the replay
                    // log needs byte-stable host values, so sort by key.
                    pairs.sort_by(|a, b| a.0.cmp(&b.0));
                    HostValue::Object(pairs)
                }
                _ => {
                    return Err(JSError::Message(
                        "host boundary: cannot convert a function to a host value".to_string(),
                    ));
                }
            };
            visiting.remove(&id.0);
            out
        }
        JSValue::Function(_) | JSValue::Handler(_) | JSValue::Continuation(_, _) => {
            return Err(JSError::Message(
                "host boundary: cannot convert code (function/handler/continuation) to a host value"
                    .to_string(),
            ));
        }
    })
}

/// Materialize a host value into the runtime's arenas.
pub fn from_host_value(interp: &mut CEKH, hv: &HostValue) -> JSValue {
    match hv {
        HostValue::Undefined => JSValue::Undefined,
        HostValue::Null => JSValue::Null,
        HostValue::Bool(b) => JSValue::Bool(*b),
        HostValue::Int(i) => JSValue::Int(*i),
        HostValue::Float(f) => JSValue::Float(*f),
        HostValue::Str(s) => JSValue::String(interp.strings.intern(s)),
        HostValue::Array(items) => {
            let elements: Vec<JSValue> =
                items.iter().map(|el| from_host_value(interp, el)).collect();
            let mut obj = Object::array();
            if let ObjectKind::Array(data) = &mut obj.kind {
                data.elements = elements;
            }
            let id = interp.objects.alloc(obj);
            JSValue::Array(ObjId(id.index() as u32))
        }
        HostValue::Object(pairs) => {
            let values: Vec<(String, JSValue)> = pairs
                .iter()
                .map(|(k, v)| (k.clone(), from_host_value(interp, v)))
                .collect();
            let mut obj = Object::new();
            for (k, v) in values {
                let key = interp.strings.intern(&k);
                obj.set(key, v);
            }
            let id = interp.objects.alloc(obj);
            JSValue::Object(ObjId(id.index() as u32))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::Runtime;

    #[test]
    fn scalars_round_trip() {
        let mut rt = Runtime::new();
        let interp = &mut rt.interpreter;
        for hv in [
            HostValue::Undefined,
            HostValue::Null,
            HostValue::Bool(true),
            HostValue::Int(-7),
            HostValue::Float(2.5),
            HostValue::Str("hello".to_string()),
        ] {
            let js = from_host_value(interp, &hv);
            assert_eq!(to_host_value(interp, js).unwrap(), hv);
        }
    }

    #[test]
    fn nested_structures_round_trip() {
        let mut rt = Runtime::new();
        let interp = &mut rt.interpreter;
        let hv = HostValue::Object(vec![
            (
                "items".to_string(),
                HostValue::Array(vec![HostValue::Int(1), HostValue::Str("two".to_string())]),
            ),
            ("ok".to_string(), HostValue::Bool(false)),
        ]);
        let js = from_host_value(interp, &hv);
        assert_eq!(to_host_value(interp, js).unwrap(), hv);
    }

    #[test]
    fn object_keys_are_sorted_deterministically() {
        let mut rt = Runtime::new();
        let v = rt.eval("let o = {zeta: 1, alpha: 2}; o").unwrap();
        let hv = to_host_value(&rt.interpreter, v).unwrap();
        let HostValue::Object(pairs) = hv else {
            panic!("expected object, got {hv:?}")
        };
        let keys: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["alpha", "zeta"]);
    }

    #[test]
    fn function_cannot_cross() {
        let mut rt = Runtime::new();
        let v = rt.eval("let f = (x) => x; f").unwrap();
        let err = to_host_value(&rt.interpreter, v).unwrap_err();
        assert!(err.to_string().contains("cannot convert"), "{err}");
    }

    #[test]
    fn cyclic_object_errors_instead_of_hanging() {
        let mut rt = Runtime::new();
        let v = rt.eval("let o = {}; o.me = o; o").unwrap();
        let err = to_host_value(&rt.interpreter, v).unwrap_err();
        assert!(err.to_string().contains("cyclic"), "{err}");
    }
}
