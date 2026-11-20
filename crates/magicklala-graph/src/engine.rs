use std::sync::Arc;

use crate::{
    EventEmitter, KnowledgeService, LlmGateway, MemoryService, NodeExecutor, NodeId, NodeSpec, RunSummary, SpellError, SpellGraph, SpellValidator, Value,
};

pub trait NodeCatalog: Send + Sync {
    fn executor(&self, kind: &str) -> Option<Arc<dyn NodeExecutor>>;
    fn specs(&self) -> Vec<NodeSpec>;
}

#[derive(Clone, Default)]
pub struct EngineServices {
    pub llm: Option<Arc<dyn LlmGateway>>,
    pub memory: Option<Arc<dyn MemoryService>>,
    pub knowledge: Option<Arc<dyn KnowledgeService>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunTrigger {
    pub node: NodeId,
    pub socket: String,
    pub payload: Value,
}

pub struct SpellEngine {
    catalog: Arc<dyn NodeCatalog>,
    services: EngineServices,
    step_limit: u64,
}

impl SpellEngine {
    pub const DEFAULT_STEP_LIMIT: u64 = 10_000;

    pub fn new(catalog: Arc<dyn NodeCatalog>, services: EngineServices) -> Self {
        Self { catalog, services, step_limit: Self::DEFAULT_STEP_LIMIT }
    }

    pub fn with_step_limit(mut self, step_limit: u64) -> Self {
        self.step_limit = step_limit;
        self
    }

    pub fn step_limit(&self) -> u64 {
        self.step_limit
    }

    pub fn services(&self) -> &EngineServices {
        &self.services
    }

    pub fn validator(&self) -> SpellValidator {
        SpellValidator::new(self.catalog.specs())
    }

    pub async fn run(&self, graph: &SpellGraph, _trigger: RunTrigger, _events: Arc<dyn EventEmitter>) -> Result<RunSummary, SpellError> {
        self.validator().validate(graph).map_err(SpellError::Validation)?;
        Err(SpellError::NotImplemented("SpellEngine::run"))
    }
}
