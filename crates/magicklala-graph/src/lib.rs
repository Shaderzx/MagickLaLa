mod engine;
mod error;
mod ir;
mod services;
mod spec;
mod trace;
mod validator;
mod value;

pub use engine::{EngineServices, NodeCatalog, RunTrigger, SpellEngine};
pub use error::{NodeError, SpellError, ValidationError};
pub use ir::{NodeId, NodeInstance, SPELL_SCHEMA_VERSION, SpellGraph, SpellId, SubgraphRef, VariableDef, Wire, WireId};
pub use services::{
    EventEmitter, KnowledgeHit, KnowledgeService, LlmGateway, LlmMessage, LlmRequest, LlmResponse, LlmRole, LlmStream, LlmStreamChunk, MemoryItem,
    MemoryService, NodeContext, NodeExecutor, NodeOutcome, TokenUsage,
};
pub use spec::{FieldKind, NodeConfigField, NodeSpec, SocketDirection, SocketSpec, SocketType};
pub use trace::{RunId, RunStatus, RunSummary, TraceEvent};
pub use validator::SpellValidator;
pub use value::Value;
