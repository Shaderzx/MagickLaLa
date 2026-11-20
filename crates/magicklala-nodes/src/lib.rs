use std::collections::BTreeMap;
use std::sync::Arc;

use magicklala_graph::{NodeCatalog, NodeExecutor, NodeSpec};

#[derive(Clone, Default)]
pub struct NodeRegistry {
    executors: BTreeMap<String, Arc<dyn NodeExecutor>>,
}

impl NodeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, executor: Arc<dyn NodeExecutor>) -> Option<Arc<dyn NodeExecutor>> {
        self.executors.insert(executor.spec().kind.clone(), executor)
    }

    pub fn get(&self, kind: &str) -> Option<Arc<dyn NodeExecutor>> {
        self.executors.get(kind).cloned()
    }

    pub fn kinds(&self) -> impl Iterator<Item = &str> {
        self.executors.keys().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.executors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.executors.is_empty()
    }
}

impl NodeCatalog for NodeRegistry {
    fn executor(&self, kind: &str) -> Option<Arc<dyn NodeExecutor>> {
        self.get(kind)
    }

    fn specs(&self) -> Vec<NodeSpec> {
        self.executors.values().map(|executor| executor.spec().clone()).collect()
    }
}

pub fn default_registry() -> NodeRegistry {
    NodeRegistry::new()
}
