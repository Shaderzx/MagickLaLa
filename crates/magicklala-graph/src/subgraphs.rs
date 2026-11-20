use std::collections::HashMap;
use std::sync::Arc;

use crate::{SpellError, SpellGraph, SpellId, SpellValidator, SubgraphRef, SubgraphResolver, ValidationError};

pub const MAX_SUBGRAPH_DEPTH: usize = 16;

pub type SubgraphKey = (SpellId, Option<u32>);
pub type ResolvedSubgraphs = HashMap<SubgraphKey, Arc<SpellGraph>>;

struct Frame {
    key: Option<SubgraphKey>,
    spell: SpellId,
    references: Vec<SubgraphRef>,
    cursor: usize,
    height: usize,
}

impl Frame {
    fn new(key: Option<SubgraphKey>, graph: &SpellGraph) -> Self {
        Self { key, spell: graph.id, references: graph.subgraphs.clone(), cursor: 0, height: 0 }
    }

    fn reach(&mut self, child_height: usize) {
        self.height = self.height.max(child_height + 1);
    }
}

fn key_of(reference: &SubgraphRef) -> SubgraphKey {
    (reference.spell_id, reference.release_version)
}

fn too_deep(reference: &SubgraphRef, levels: usize) -> SpellError {
    SpellError::Internal(format!("subgraph nesting through node {:?} reaches {levels} levels and exceeds {MAX_SUBGRAPH_DEPTH} levels", reference.node_id.0))
}

fn cycle_through(frames: &[Frame], reference: &SubgraphRef) -> Option<SpellError> {
    let start = frames.iter().position(|frame| frame.spell == reference.spell_id)?;
    let chain: Vec<String> =
        frames[start..].iter().map(|frame| frame.spell).chain(std::iter::once(reference.spell_id)).map(|spell| spell.0.to_string()).collect();
    Some(SpellError::Internal(format!("subgraph cycle through node {:?}: {}", reference.node_id.0, chain.join(" -> "))))
}

fn unresolved(reference: &SubgraphRef) -> SpellError {
    SpellError::Internal(format!("subgraph {:?} for node {:?} cannot be resolved", reference.spell_id, reference.node_id))
}

fn walk_subgraphs(
    root: &SpellGraph,
    resolver: &dyn SubgraphResolver,
    mut check: impl FnMut(&SpellGraph) -> Result<(), SpellError>,
) -> Result<ResolvedSubgraphs, SpellError> {
    let mut resolved = ResolvedSubgraphs::new();
    let mut heights: HashMap<SubgraphKey, usize> = HashMap::new();
    let mut frames = vec![Frame::new(None, root)];
    loop {
        let depth = frames.len().saturating_sub(1);
        let Some(frame) = frames.last_mut() else {
            return Ok(resolved);
        };
        let next = frame.references.get(frame.cursor).cloned();
        frame.cursor += 1;
        let Some(reference) = next else {
            let Some(done) = frames.pop() else {
                return Ok(resolved);
            };
            if let Some(key) = done.key {
                heights.insert(key, done.height);
            }
            if let Some(parent) = frames.last_mut() {
                parent.reach(done.height);
            }
            continue;
        };
        if let Some(cycle) = cycle_through(&frames, &reference) {
            return Err(cycle);
        }
        let key = key_of(&reference);
        if let Some(height) = heights.get(&key).copied() {
            let levels = depth + 1 + height;
            if levels > MAX_SUBGRAPH_DEPTH {
                return Err(too_deep(&reference, levels));
            }
            if let Some(frame) = frames.last_mut() {
                frame.reach(height);
            }
            continue;
        }
        if depth + 1 > MAX_SUBGRAPH_DEPTH {
            return Err(too_deep(&reference, depth + 1));
        }
        let child = resolver.resolve(reference.spell_id, reference.release_version).ok_or_else(|| unresolved(&reference))?;
        check(&child)?;
        frames.push(Frame::new(Some(key), &child));
        resolved.insert(key, Arc::new(child));
    }
}

pub fn resolve_subgraphs(root: &SpellGraph, resolver: &dyn SubgraphResolver, validator: &SpellValidator) -> Result<ResolvedSubgraphs, SpellError> {
    walk_subgraphs(root, resolver, |child| validator.validate(child).map_err(SpellError::Validation))
}

pub fn verify_subgraphs(root: &SpellGraph, resolver: &dyn SubgraphResolver) -> Result<(), SpellError> {
    walk_subgraphs(root, resolver, |_| Ok(())).map(drop)
}

pub(crate) fn unresolvable_subgraphs(graph: &SpellGraph) -> Result<(), SpellError> {
    if graph.subgraphs.is_empty() {
        return Ok(());
    }
    Err(SpellError::Validation(
        graph
            .subgraphs
            .iter()
            .map(|reference| ValidationError::SubgraphResolverUnavailable { node: reference.node_id.clone(), spell: reference.spell_id })
            .collect(),
    ))
}
