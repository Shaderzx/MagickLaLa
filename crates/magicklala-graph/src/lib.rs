mod engine;
mod error;
mod ir;
mod services;
mod spec;
mod subgraphs;
mod trace;
mod validator;
mod value;

pub use engine::{
    DEFAULT_WHILE_LOOP_MAX_ITERATIONS, DO_ONCE_KIND, EngineServices, MAX_ACTIVATION_LOG_BYTES, MAX_REPLIES, MAX_REPLY_BYTES, NodeCatalog, RunBinding,
    RunTrigger, START_TRIGGER_KIND, SUBGRAPH_KIND, SpellEngine, SubgraphResolver, TRIGGER_PAYLOAD_SOCKET, UNBOUND_SETTINGS, WAIT_FOR_ALL_KIND, WHILE_LOOP_KIND,
    bound_variable, whole_number,
};
pub use error::{NodeError, SpellError, ValidationError};
pub use ir::{NodeId, NodeInstance, SPELL_SCHEMA_VERSION, ShelfEntry, SpellGraph, SpellId, SubgraphRef, VariableDef, Wire, WireId};
pub use services::{
    CredentialResolver, EmbeddingRequest, EventEmitter, KnowledgeHit, KnowledgeService, LlmGateway, LlmMessage, LlmRequest, LlmResponse, LlmRole, LlmStream,
    LlmStreamChunk, MemoryItem, MemoryService, NodeContext, NodeExecutor, NodeOutcome, TokenUsage,
};
pub use spec::{DYNAMIC_SOCKET, FieldKind, NodeConfigField, NodeSpec, SocketDirection, SocketSpec, SocketType};
pub use subgraphs::{MAX_SUBGRAPH_DEPTH, ResolvedSubgraphs, SubgraphKey, resolve_subgraphs, resolve_subgraphs_from, verify_subgraphs, verify_subgraphs_from};
pub use trace::{EmittedOutput, RunId, RunStatus, RunSummary, TraceEvent};
pub use validator::SpellValidator;
pub use value::Value;
