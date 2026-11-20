use std::sync::{Mutex, MutexGuard, PoisonError};

use magicklala_graph::{RunSummary, TraceEvent};
use serde_json::{Map, Value};

pub const TRACE_EVENT_CAP: usize = 256;
pub const TRACE_BYTE_CAP: usize = 64 * 1024;
pub const TRACE_ERROR_CAP: usize = 2 * 1024;
pub const TRACE_FIELD_CAP: usize = 256;

const TRACE_ENVELOPE_CAP: usize = 8 * 1024;
const RUN_FINISHED: &str = "run_finished";
const EVENTS_KEY: &str = "events";
const TRUNCATED_KEY: &str = "truncated";

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

fn encoded_len(text: &str) -> usize {
    serialized_len(&text)
}

pub fn clamp_encoded(text: &str, max_encoded_bytes: usize) -> String {
    if encoded_len(text) <= max_encoded_bytes {
        return text.to_owned();
    }
    let boundaries: Vec<usize> = text.char_indices().map(|(index, _)| index).chain(std::iter::once(text.len())).collect();
    let fitting = boundaries.partition_point(|cut| encoded_len(&text[..*cut]) <= max_encoded_bytes);
    let cut = fitting.checked_sub(1).and_then(|index| boundaries.get(index)).copied().unwrap_or(0);
    text[..cut].to_owned()
}

pub fn trace_summary(summary: &RunSummary) -> RunSummary {
    let mut slim = summary.slim();
    slim.error = slim.error.map(|error| clamp_encoded(&clamp_text(&error, TRACE_ERROR_CAP), TRACE_ERROR_CAP));
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

fn clamp_value(value: Value) -> Value {
    match value {
        Value::String(text) => Value::String(clamp_encoded(&text, TRACE_FIELD_CAP)),
        Value::Array(_) | Value::Object(_) if serialized_len(&value) > TRACE_FIELD_CAP => Value::Null,
        other => other,
    }
}

fn clamp_envelope(envelope: Map<String, Value>) -> Map<String, Value> {
    let mut kept = Map::new();
    let mut used = 2_usize;
    for (key, value) in envelope.into_iter().filter(|(key, _)| key != EVENTS_KEY && key != TRUNCATED_KEY) {
        let (key, value) = (clamp_encoded(&key, TRACE_FIELD_CAP), clamp_value(value));
        let entry = encoded_len(&key) + 1 + serialized_len(&value) + 1;
        if used + entry > TRACE_ENVELOPE_CAP {
            break;
        }
        used += entry;
        kept.insert(key, value);
    }
    kept
}

fn serialized_len(value: &impl serde::Serialize) -> usize {
    serde_json::to_string(value).map_or(usize::MAX, |text| text.len())
}

fn sized(value: Value) -> SizedEvent {
    let size = serialized_len(&value);
    (value, size)
}

fn with_error(terminal: &Value, error: Value) -> Option<SizedEvent> {
    let mut value = terminal.clone();
    value.get_mut("summary").and_then(Value::as_object_mut)?.insert("error".to_owned(), error);
    Some(sized(value))
}

fn terminal_shrunk(terminal: &SizedEvent, room: usize) -> Option<SizedEvent> {
    if terminal.1 <= room {
        return Some(terminal.clone());
    }
    let without_error = with_error(&terminal.0, Value::Null).filter(|(_, size)| *size <= room)?;
    let Some(error) = terminal.0.pointer("/summary/error").and_then(Value::as_str) else {
        return Some(without_error);
    };
    let kept = clamp_encoded(error, room - without_error.1);
    if kept.is_empty() {
        return Some(without_error);
    }
    with_error(&terminal.0, Value::String(kept)).filter(|(_, size)| *size <= room).or(Some(without_error))
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
        let Ok(value) = serde_json::to_value(compact(&TraceEvent::RunFinished { run_id: summary.run_id, summary: summary.clone() })) else { return };
        let size = serialized_len(&value);
        let mut state = self.lock();
        let stale: usize = state.events.iter().filter(|(event, _)| is_terminal(event)).map(|(_, size)| size + 1).sum();
        state.events.retain(|(event, _)| !is_terminal(event));
        state.bytes = state.bytes.saturating_sub(stale).saturating_add(size + 1);
        state.events.push((value, size));
        state.closed = true;
    }

    pub fn seal(&self, envelope: Map<String, Value>) -> Value {
        let mut object = clamp_envelope(envelope);
        object.insert(EVENTS_KEY.to_owned(), Value::Array(Vec::new()));
        object.insert(TRUNCATED_KEY.to_owned(), Value::Bool(false));
        let budget = TRACE_BYTE_CAP.saturating_sub(serialized_len(&object));
        let (events, truncated) = self.fitted(budget);
        object.insert(EVENTS_KEY.to_owned(), Value::Array(events));
        object.insert(TRUNCATED_KEY.to_owned(), Value::Bool(truncated));
        Value::Object(object)
    }

    pub fn snapshot(&self) -> (Vec<Value>, bool) {
        let sealed = self.seal(Map::new());
        let events = sealed.get(EVENTS_KEY).and_then(Value::as_array).cloned().unwrap_or_default();
        (events, sealed.get(TRUNCATED_KEY).and_then(Value::as_bool).unwrap_or(true))
    }

    fn fitted(&self, budget: usize) -> (Vec<Value>, bool) {
        let state = self.lock();
        let mut truncated = state.truncated;
        let mut used = 0_usize;
        let mut terminals = Vec::new();
        for terminal in state.events.iter().filter(|(value, _)| is_terminal(value)) {
            let separator = usize::from(!terminals.is_empty());
            let room = budget.saturating_sub(used + separator);
            match terminal_shrunk(terminal, room) {
                Some((value, size)) => {
                    used += size + separator;
                    terminals.push(value);
                }
                None => truncated = true,
            }
        }
        let room = TRACE_EVENT_CAP.saturating_sub(terminals.len());
        let mut kept = Vec::with_capacity(state.events.len().min(TRACE_EVENT_CAP));
        for (value, size) in state.events.iter().filter(|(value, _)| !is_terminal(value)) {
            let separator = usize::from(!kept.is_empty() || !terminals.is_empty());
            if kept.len() >= room || used.saturating_add(size + separator) > budget {
                truncated = true;
                break;
            }
            used += size + separator;
            kept.push(value.clone());
        }
        kept.extend(terminals);
        (kept, truncated)
    }
}
