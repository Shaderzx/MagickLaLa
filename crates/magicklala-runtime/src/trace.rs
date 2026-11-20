use std::sync::{Mutex, MutexGuard, PoisonError};

use magicklala_graph::{RunSummary, TraceEvent};
use serde_json::{Map, Value};

pub const TRACE_EVENT_CAP: usize = 256;
pub const TRACE_BYTE_CAP: usize = 64 * 1024;
pub const TRACE_ERROR_CAP: usize = 2 * 1024;
pub const TRACE_FIELD_CAP: usize = 256;
pub const TRACE_REPLY_CAP: usize = 16 * 1024;
pub const TRACE_REPLY_KEY: &str = "reply";

const RUN_FINISHED: &str = "run_finished";

type SizedEvent = (Value, usize);

#[derive(Debug, Default)]
struct TraceState {
    events: Vec<SizedEvent>,
    bytes: usize,
    truncated: bool,
    closed: bool,
}

#[derive(Debug, Default)]
pub struct BoundedTrace {
    state: Mutex<TraceState>,
}

pub fn clamp_text(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let cut = (0..=max_bytes).rev().find(|index| text.is_char_boundary(*index)).unwrap_or(0);
    text[..cut].to_owned()
}

pub fn trace_summary(summary: &RunSummary) -> RunSummary {
    let mut slim = summary.slim();
    slim.error = slim.error.map(|error| clamp_text(&error, TRACE_ERROR_CAP));
    slim
}

fn compact(event: &TraceEvent) -> TraceEvent {
    match event {
        TraceEvent::RunFinished { run_id, summary } => TraceEvent::RunFinished { run_id: *run_id, summary: trace_summary(summary) },
        other => other.clone(),
    }
}

fn is_terminal(value: &Value) -> bool {
    value.get("event").and_then(Value::as_str) == Some(RUN_FINISHED)
}

fn clamp_envelope(envelope: Map<String, Value>) -> Map<String, Value> {
    envelope
        .into_iter()
        .map(|(key, value)| {
            let limit = if key == TRACE_REPLY_KEY { TRACE_REPLY_CAP } else { TRACE_FIELD_CAP };
            let value = match value {
                Value::String(text) if text.len() > limit && key == TRACE_REPLY_KEY => Value::Null,
                Value::String(text) => Value::String(clamp_text(&text, limit)),
                other => other,
            };
            (key, value)
        })
        .collect()
}

fn serialized_len(value: &impl serde::Serialize) -> usize {
    serde_json::to_string(value).map_or(usize::MAX, |text| text.len())
}

impl BoundedTrace {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, TraceState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn push(&self, event: &TraceEvent) {
        let terminal = matches!(event, TraceEvent::RunFinished { .. });
        if !terminal {
            let mut state = self.lock();
            if state.events.len() + 1 >= TRACE_EVENT_CAP || state.bytes >= TRACE_BYTE_CAP {
                state.truncated = true;
                return;
            }
        }
        let Ok(value) = serde_json::to_value(compact(event)) else { return };
        let size = serialized_len(&value);
        let mut state = self.lock();
        let fits = state.events.len() + 1 < TRACE_EVENT_CAP && state.bytes.saturating_add(size + 1) <= TRACE_BYTE_CAP;
        if terminal || fits {
            state.bytes = state.bytes.saturating_add(size + 1);
            state.events.push((value, size));
            state.closed |= terminal;
        } else {
            state.truncated = true;
        }
    }

    pub fn closed(&self) -> bool {
        self.lock().closed
    }

    pub fn close(&self, summary: &RunSummary) {
        if !self.closed() {
            self.push(&TraceEvent::RunFinished { run_id: summary.run_id, summary: summary.clone() });
        }
    }

    pub fn seal(&self, envelope: Map<String, Value>) -> Value {
        let mut object = clamp_envelope(envelope);
        object.insert("events".to_owned(), Value::Array(Vec::new()));
        object.insert("truncated".to_owned(), Value::Bool(false));
        let budget = TRACE_BYTE_CAP.saturating_sub(serialized_len(&object));
        let (events, truncated) = self.fitted(budget);
        object.insert("events".to_owned(), Value::Array(events));
        object.insert("truncated".to_owned(), Value::Bool(truncated));
        Value::Object(object)
    }

    pub fn snapshot(&self) -> (Vec<Value>, bool) {
        let sealed = self.seal(Map::new());
        let events = sealed.get("events").and_then(Value::as_array).cloned().unwrap_or_default();
        (events, sealed.get("truncated").and_then(Value::as_bool).unwrap_or(true))
    }

    fn fitted(&self, budget: usize) -> (Vec<Value>, bool) {
        let state = self.lock();
        let (terminals, others): (Vec<&SizedEvent>, Vec<&SizedEvent>) = state.events.iter().partition(|(value, _)| is_terminal(value));
        let mut used: usize = terminals.iter().map(|(_, size)| size + 1).sum();
        let room = TRACE_EVENT_CAP.saturating_sub(terminals.len());
        let mut truncated = state.truncated;
        let mut kept = Vec::with_capacity(state.events.len().min(TRACE_EVENT_CAP));
        for (value, size) in others {
            if kept.len() >= room || used.saturating_add(size + 1) > budget {
                truncated = true;
                break;
            }
            used += size + 1;
            kept.push(value.clone());
        }
        kept.extend(terminals.into_iter().map(|(value, _)| value.clone()));
        (kept, truncated)
    }
}
