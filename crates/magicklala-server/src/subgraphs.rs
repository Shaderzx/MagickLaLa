use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use async_trait::async_trait;
use magicklala_graph::{SpellGraph, SpellId, SpellValidator, SubgraphResolver};
use magicklala_runtime::SubgraphLoader;
use magicklala_store::{ProjectId, Repository, StoreResult};

pub const MAX_SUBGRAPH_LOOKUPS: usize = 64;

type SubgraphKey = (SpellId, Option<u32>);

#[derive(Debug, Clone, Default)]
pub struct ProjectSubgraphResolver {
    graphs: HashMap<SubgraphKey, SpellGraph>,
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
            let Some(record) = spells.iter().find(|spell| spell.id.0 == spell_id.0 || spell.graph.id == spell_id) else { continue };
            let graph = match version {
                None => Some(record.graph.clone()),
                Some(version) => store.list_releases(record.id).await?.into_iter().find(|release| release.version == version).map(|release| release.graph),
            };
            if let Some(graph) = graph {
                pending.extend(references(&graph));
                graphs.insert(key, graph);
            }
        }
        Ok(Self { graphs })
    }

    pub fn verify(&self, root: &SpellGraph, validator: &SpellValidator) -> Result<(), String> {
        let mut path = vec![root.id];
        let mut finished = HashSet::new();
        self.visit(root, validator, &mut path, &mut finished)
    }

    fn visit(&self, graph: &SpellGraph, validator: &SpellValidator, path: &mut Vec<SpellId>, finished: &mut HashSet<SubgraphKey>) -> Result<(), String> {
        for reference in &graph.subgraphs {
            let key = (reference.spell_id, reference.release_version);
            if let Some(start) = path.iter().position(|ancestor| *ancestor == reference.spell_id) {
                let chain: Vec<String> = path[start..].iter().chain(std::iter::once(&reference.spell_id)).map(|spell| spell.0.to_string()).collect();
                return Err(format!("subgraph cycle through node {:?}: {}", reference.node_id.0, chain.join(" -> ")));
            }
            if finished.contains(&key) {
                continue;
            }
            let child = self
                .graphs
                .get(&key)
                .ok_or_else(|| format!("subgraph {} for node {:?} cannot be resolved in this project", reference.spell_id.0, reference.node_id.0))?;
            validator.validate(child).map_err(|errors| {
                format!("subgraph {} is invalid: {}", reference.spell_id.0, errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))
            })?;
            path.push(reference.spell_id);
            let nested = self.visit(child, validator, path, finished);
            path.pop();
            nested?;
            finished.insert(key);
        }
        Ok(())
    }
}

impl SubgraphResolver for ProjectSubgraphResolver {
    fn resolve(&self, spell_id: SpellId, release_version: Option<u32>) -> Option<SpellGraph> {
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
    async fn load(&self, project: ProjectId, graph: &SpellGraph) -> Arc<dyn SubgraphResolver> {
        Arc::new(ProjectSubgraphResolver::load(self.store.as_ref(), project, graph).await.unwrap_or_default())
    }
}
