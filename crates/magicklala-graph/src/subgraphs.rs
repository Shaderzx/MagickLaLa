use std::collections::HashMap;
use std::sync::Arc;

use crate::{SpellError, SpellGraph, SpellId, SpellValidator, SubgraphRef, SubgraphResolver, ValidationError};

pub const MAX_SUBGRAPH_DEPTH: usize = 16;

pub type SubgraphKey = (SpellId, Option<u32>);
pub type ResolvedSubgraphs = HashMap<SubgraphKey, Arc<SpellGraph>>;

struct Frame {
    key: SubgraphKey,
    references: Vec<SubgraphRef>,
    cursor: usize,
    height: usize,
}

impl Frame {
    fn new(key: SubgraphKey, graph: &SpellGraph) -> Self {
        Self { key, references: graph.subgraphs.clone(), cursor: 0, height: 0 }
    }

    fn reach(&mut self, child_height: usize) {
        self.height = self.height.max(child_height + 1);
    }
}

fn lookup_key(reference: &SubgraphRef) -> SubgraphKey {
    (reference.spell_id, reference.release_version)
}

fn snapshot_name((spell, version): SubgraphKey) -> String {
    match version {
        Some(version) => format!("{}@{version}", spell.0),
        None => spell.0.to_string(),
    }
}

fn too_deep(reference: &SubgraphRef, levels: usize) -> SpellError {
    SpellError::Internal(format!("subgraph nesting through node {:?} reaches {levels} levels and exceeds {MAX_SUBGRAPH_DEPTH} levels", reference.node_id.0))
}

fn cycle_through(frames: &[Frame], snapshot: SubgraphKey, reference: &SubgraphRef) -> Option<SpellError> {
    let start = frames.iter().position(|frame| frame.key == snapshot)?;
    let chain: Vec<String> = frames[start..].iter().map(|frame| frame.key).chain(std::iter::once(snapshot)).map(snapshot_name).collect();
    Some(SpellError::Internal(format!("subgraph cycle through node {:?}: {}", reference.node_id.0, chain.join(" -> "))))
}

fn unresolved(reference: &SubgraphRef) -> SpellError {
    SpellError::Internal(format!("subgraph {:?} for node {:?} cannot be resolved", reference.spell_id, reference.node_id))
}

type CanonicalChildren = HashMap<SubgraphKey, (SpellId, Arc<SpellGraph>)>;

fn resolve_child(
    known: &mut CanonicalChildren,
    resolver: &dyn SubgraphResolver,
    reference: &SubgraphRef,
) -> Result<(SubgraphKey, Arc<SpellGraph>), SpellError> {
    let lookup = lookup_key(reference);
    if let Some((canonical, child)) = known.get(&lookup) {
        return Ok(((*canonical, reference.release_version), Arc::clone(child)));
    }
    let (canonical, child) = resolver.resolve_entry(reference.spell_id, reference.release_version).ok_or_else(|| unresolved(reference))?;
    let child = Arc::new(child);
    known.insert(lookup, (canonical, Arc::clone(&child)));
    Ok(((canonical, reference.release_version), child))
}

fn walk_subgraphs(
    root: &SpellGraph,
    root_key: SubgraphKey,
    resolver: &dyn SubgraphResolver,
    mut check: impl FnMut(&SpellGraph) -> Result<(), SpellError>,
) -> Result<ResolvedSubgraphs, SpellError> {
    let mut resolved = ResolvedSubgraphs::new();
    let mut known = CanonicalChildren::new();
    let mut heights: HashMap<SubgraphKey, usize> = HashMap::new();
    let mut frames = vec![Frame::new(root_key, root)];
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
            heights.insert(done.key, done.height);
            if let Some(parent) = frames.last_mut() {
                parent.reach(done.height);
            }
            continue;
        };
        let (snapshot, child) = resolve_child(&mut known, resolver, &reference)?;
        if let Some(cycle) = cycle_through(&frames, snapshot, &reference) {
            return Err(cycle);
        }
        if let Some(height) = heights.get(&snapshot).copied() {
            let levels = depth + 1 + height;
            if levels > MAX_SUBGRAPH_DEPTH {
                return Err(too_deep(&reference, levels));
            }
            if let Some(frame) = frames.last_mut() {
                frame.reach(height);
            }
            resolved.entry(lookup_key(&reference)).or_insert(child);
            continue;
        }
        if depth + 1 > MAX_SUBGRAPH_DEPTH {
            return Err(too_deep(&reference, depth + 1));
        }
        check(&child)?;
        frames.push(Frame::new(snapshot, &child));
        resolved.insert(lookup_key(&reference), child);
    }
}

pub fn resolve_subgraphs_from(
    root: &SpellGraph,
    root_key: SubgraphKey,
    resolver: &dyn SubgraphResolver,
    validator: &SpellValidator,
) -> Result<ResolvedSubgraphs, SpellError> {
    walk_subgraphs(root, root_key, resolver, |child| validator.validate(child).map_err(SpellError::Validation))
}

pub fn resolve_subgraphs(root: &SpellGraph, resolver: &dyn SubgraphResolver, validator: &SpellValidator) -> Result<ResolvedSubgraphs, SpellError> {
    resolve_subgraphs_from(root, (root.id, None), resolver, validator)
}

pub fn verify_subgraphs_from(root: &SpellGraph, root_key: SubgraphKey, resolver: &dyn SubgraphResolver) -> Result<(), SpellError> {
    walk_subgraphs(root, root_key, resolver, |_| Ok(())).map(drop)
}

pub fn verify_subgraphs(root: &SpellGraph, resolver: &dyn SubgraphResolver) -> Result<(), SpellError> {
    verify_subgraphs_from(root, (root.id, None), resolver)
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
