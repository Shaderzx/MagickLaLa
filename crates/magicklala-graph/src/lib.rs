mod engine;
mod error;
mod ir;
mod services;
mod spec;
mod trace;
mod validator;
mod value;

pub use engine::{
    DEFAULT_WHILE_LOOP_MAX_ITERATIONS, DO_ONCE_KIND, EngineServices, MAX_ACTIVATION_LOG_BYTES, NodeCatalog, RunBinding, RunTrigger, SpellEngine,
    SubgraphResolver, TRIGGER_PAYLOAD_SOCKET, WAIT_FOR_ALL_KIND, WHILE_LOOP_KIND,
};
pub use error::{NodeError, SpellError, ValidationError};
pub use ir::{NodeId, NodeInstance, SPELL_SCHEMA_VERSION, SpellGraph, SpellId, SubgraphRef, VariableDef, Wire, WireId};
pub use services::{
    CredentialResolver, EmbeddingRequest, EventEmitter, KnowledgeHit, KnowledgeService, LlmGateway, LlmMessage, LlmRequest, LlmResponse, LlmRole, LlmStream,
    LlmStreamChunk, MemoryItem, MemoryService, NodeContext, NodeExecutor, NodeOutcome, TokenUsage,
};
pub use spec::{DYNAMIC_SOCKET, FieldKind, NodeConfigField, NodeSpec, SocketDirection, SocketSpec, SocketType};
pub use trace::{EmittedOutput, RunId, RunStatus, RunSummary, TraceEvent};
pub use validator::SpellValidator;
pub use value::Value;
