use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use async_trait::async_trait;
use magicklala_graph::{SpellGraph, SpellId, SpellValidator, SubgraphKey, SubgraphResolver};
use magicklala_runtime::SubgraphLoader;
use magicklala_store::{ProjectId, Repository, StoreResult};

pub const MAX_SUBGRAPH_LOOKUPS: usize = 64;

#[derive(Debug, Clone, Default)]
pub struct ProjectSubgraphResolver {
    graphs: HashMap<SubgraphKey, (SpellId, SpellGraph)>,
}

fn references(graph: &SpellGraph) -> impl Iterator<Item = SubgraphKey> + '_ {
    graph.subgraphs.iter().map(|reference| (reference.spell_id, reference.release_version))
}

impl ProjectSubgraphResolver {
    pub async fn load(store: &dyn Repository, project: ProjectId, root: &SpellGraph) -> StoreResult<Self> {
        let mut graphs = HashMap::new();
        if root.subgraphs.is_empty() {
            return Ok(Self { graphs });
        }
        let spells = store.list_spells(project).await?;
        let mut pending: VecDeque<SubgraphKey> = references(root).collect();
        let mut seen = HashSet::new();
        while let Some(key) = pending.pop_front() {
            if seen.len() >= MAX_SUBGRAPH_LOOKUPS {
                break;
            }
            if !seen.insert(key) {
                continue;
            }
            let (spell_id, version) = key;
            let by_record = spells.iter().find(|spell| spell.id.0 == spell_id.0);
            let Some(record) = by_record.or_else(|| spells.iter().find(|spell| spell.graph.id == spell_id)) else { continue };
            let graph = match version {
                None => Some(record.graph.clone()),
                Some(version) => store.list_releases(record.id).await?.into_iter().find(|release| release.version == version).map(|release| release.graph),
            };
            if let Some(graph) = graph {
                pending.extend(references(&graph));
                graphs.insert(key, (SpellId(record.id.0), graph));
            }
        }
        Ok(Self { graphs })
    }

    pub fn verify_from(&self, root: &SpellGraph, root_key: SubgraphKey, validator: &SpellValidator) -> Result<(), String> {
        magicklala_graph::resolve_subgraphs_from(root, root_key, self, validator).map(drop).map_err(|error| error.to_string())
    }
}

impl SubgraphResolver for ProjectSubgraphResolver {
    fn resolve(&self, spell_id: SpellId, release_version: Option<u32>) -> Option<SpellGraph> {
        self.resolve_entry(spell_id, release_version).map(|(_, graph)| graph)
    }

    fn resolve_entry(&self, spell_id: SpellId, release_version: Option<u32>) -> Option<(SpellId, SpellGraph)> {
        self.graphs.get(&(spell_id, release_version)).cloned()
    }
}

pub struct StoreSubgraphLoader {
    store: Arc<dyn Repository>,
}

impl StoreSubgraphLoader {
    pub fn new(store: Arc<dyn Repository>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl SubgraphLoader for StoreSubgraphLoader {
    async fn load(&self, project: ProjectId, graph: &SpellGraph) -> Result<Arc<dyn SubgraphResolver>, String> {
        match ProjectSubgraphResolver::load(self.store.as_ref(), project, graph).await {
            Ok(resolver) => Ok(Arc::new(resolver)),
            Err(error) => {
                tracing::warn!(project_id = %project.0, spell_id = %graph.id.0, %error, "subgraph resolver could not load the spell's subgraphs from the store");
                Err(error.to_string())
            }
        }
    }
}
