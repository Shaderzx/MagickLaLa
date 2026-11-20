use std::sync::{Mutex, PoisonError};

use magicklala_graph::TraceEvent;
use serde_json::Value;

pub const TRACE_EVENT_CAP: usize = 256;
pub const TRACE_BYTE_CAP: usize = 64 * 1024;

#[derive(Debug, Default)]
struct TraceState {
    events: Vec<Value>,
    bytes: usize,
    truncated: bool,
}

#[derive(Debug, Default)]
pub struct BoundedTrace {
    state: Mutex<TraceState>,
}

impl BoundedTrace {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TraceState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn push(&self, event: &TraceEvent) {
        let terminal = matches!(event, TraceEvent::RunFinished { .. });
        if !terminal && self.lock().events.len() >= TRACE_EVENT_CAP {
            self.lock().truncated = true;
            return;
        }
        let Ok(value) = serde_json::to_value(event) else { return };
        let size = value.to_string().len();
        let mut state = self.lock();
        let fits = state.events.len() < TRACE_EVENT_CAP && state.bytes.saturating_add(size) <= TRACE_BYTE_CAP;
        if terminal || fits {
            state.bytes = state.bytes.saturating_add(size);
            state.events.push(value);
        } else {
            state.truncated = true;
        }
    }

    pub fn snapshot(&self) -> (Vec<Value>, bool) {
        let state = self.lock();
        (state.events.clone(), state.truncated)
    }
}
