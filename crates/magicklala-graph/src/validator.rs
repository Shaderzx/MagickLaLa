use std::collections::{BTreeMap, BTreeSet};

use crate::{NodeId, NodeInstance, NodeSpec, SPELL_SCHEMA_VERSION, SUBGRAPH_KIND, SpellGraph, ValidationError, WAIT_FOR_ALL_KIND, Wire, WireId};

const CONDITIONAL_KINDS: [&str; 2] = ["flow/branch", "flow/switch"];

pub struct SpellValidator {
    specs: BTreeMap<String, NodeSpec>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Visit {
    InProgress,
    Done,
}

struct GraphIndex<'g> {
    graph: &'g SpellGraph,
    nodes: BTreeMap<&'g NodeId, &'g NodeInstance>,
    wires_into: BTreeMap<&'g NodeId, Vec<&'g Wire>>,
}

impl<'g> GraphIndex<'g> {
    fn new(graph: &'g SpellGraph) -> Self {
        let mut nodes = BTreeMap::new();
        for node in &graph.nodes {
            nodes.entry(&node.id).or_insert(node);
        }
        let mut wires_into: BTreeMap<&NodeId, Vec<&Wire>> = BTreeMap::new();
        for wire in &graph.wires {
            wires_into.entry(&wire.to_node).or_default().push(wire);
        }
        Self { graph, nodes, wires_into }
    }

    fn node(&self, id: &NodeId) -> Option<&'g NodeInstance> {
        self.nodes.get(id).copied()
    }

    fn wires_into(&self, id: &NodeId) -> &[&'g Wire] {
        self.wires_into.get(id).map_or(&[], Vec::as_slice)
    }
}

impl SpellValidator {
    pub fn new(specs: impl IntoIterator<Item = NodeSpec>) -> Self {
        Self { specs: specs.into_iter().map(|spec| (spec.kind.clone(), spec)).collect() }
    }

    pub fn spec(&self, kind: &str) -> Option<&NodeSpec> {
        self.specs.get(kind)
    }

    pub fn validate(&self, graph: &SpellGraph) -> Result<(), Vec<ValidationError>> {
        let index = GraphIndex::new(graph);
        let mut errors = Vec::new();
        if graph.schema_version != SPELL_SCHEMA_VERSION {
            errors.push(ValidationError::SchemaVersion { found: graph.schema_version, expected: SPELL_SCHEMA_VERSION });
        }
        self.check_nodes(graph, &mut errors);
        check_variables(graph, &mut errors);
        let value_edges = self.check_wires(&index, &mut errors);
        self.check_required_inputs(&index, &mut errors);
        self.check_exclusive_joins(&index, &mut errors);
        for cycle in find_cycles(graph, &value_edges) {
            errors.push(ValidationError::ValueCycle(cycle));
        }
        if !graph.nodes.iter().any(|node| self.specs.get(&node.kind).is_some_and(|spec| spec.is_trigger)) {
            errors.push(ValidationError::NoTrigger);
        }
        for subgraph in graph.subgraphs.iter().filter(|subgraph| subgraph.spell_id == graph.id) {
            errors.push(ValidationError::SubgraphCycle { node: subgraph.node_id.clone(), spell: subgraph.spell_id });
        }
        check_subgraph_calls(&index, &mut errors);
        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }

    fn check_nodes(&self, graph: &SpellGraph, errors: &mut Vec<ValidationError>) {
        let mut seen = BTreeSet::new();
        for node in &graph.nodes {
            if !seen.insert(&node.id) {
                errors.push(ValidationError::DuplicateNode(node.id.clone()));
            }
            if !self.specs.contains_key(&node.kind) {
                errors.push(ValidationError::UnknownNodeKind { node: node.id.clone(), kind: node.kind.clone() });
            }
        }
    }

    fn spec_of(&self, index: &GraphIndex<'_>, node: &NodeId) -> Option<&NodeSpec> {
        index.node(node).and_then(|instance| self.specs.get(&instance.kind))
    }

    fn check_wires<'g>(&self, index: &GraphIndex<'g>, errors: &mut Vec<ValidationError>) -> BTreeMap<&'g NodeId, Vec<&'g NodeId>> {
        let mut wire_ids = BTreeSet::new();
        let mut value_inputs: BTreeMap<(&NodeId, &str), usize> = BTreeMap::new();
        let mut value_edges: BTreeMap<&NodeId, Vec<&NodeId>> = BTreeMap::new();
        for wire in &index.graph.wires {
            if !wire_ids.insert(&wire.id) {
                errors.push(ValidationError::DuplicateWire(wire.id.clone()));
            }
            let mut endpoints_exist = true;
            for endpoint in [&wire.from_node, &wire.to_node] {
                if index.node(endpoint).is_none() {
                    errors.push(ValidationError::MissingNode { wire: wire.id.clone(), node: endpoint.clone() });
                    endpoints_exist = false;
                }
            }
            if !endpoints_exist {
                continue;
            }
            let (Some(from_spec), Some(to_spec)) = (self.spec_of(index, &wire.from_node), self.spec_of(index, &wire.to_node)) else {
                continue;
            };
            let from = from_spec.resolve_output(&wire.from_socket);
            let to = to_spec.resolve_input(&wire.to_socket);
            if from.is_none() {
                errors.push(ValidationError::MissingSocket { wire: wire.id.clone(), node: wire.from_node.clone(), socket: wire.from_socket.clone() });
            }
            if to.is_none() {
                errors.push(ValidationError::MissingSocket { wire: wire.id.clone(), node: wire.to_node.clone(), socket: wire.to_socket.clone() });
            }
            let (Some(from), Some(to)) = (from, to) else {
                continue;
            };
            if from.ty.is_flow() != to.ty.is_flow() || !to.ty.accepts(from.ty) {
                errors.push(ValidationError::TypeMismatch { wire: wire.id.clone(), from: from.ty, to: to.ty });
                continue;
            }
            if to.ty.is_flow() {
                continue;
            }
            let count = value_inputs.entry((&wire.to_node, wire.to_socket.as_str())).or_insert(0);
            *count += 1;
            if *count == 2 {
                errors.push(ValidationError::MultipleValueInputs { node: wire.to_node.clone(), socket: wire.to_socket.clone() });
            }
            value_edges.entry(&wire.from_node).or_default().push(&wire.to_node);
        }
        value_edges
    }

    fn incoming_flow<'g>(&self, index: &GraphIndex<'g>, node: &NodeId) -> Vec<&'g Wire> {
        let Some(spec) = self.spec_of(index, node) else {
            return Vec::new();
        };
        index.wires_into(node).iter().copied().filter(|wire| spec.resolve_input(&wire.to_socket).is_some_and(|socket| socket.ty.is_flow())).collect()
    }

    fn conditional_guard<'g>(&self, index: &GraphIndex<'g>, wire: &'g Wire) -> Option<(&'g NodeId, &'g str)> {
        let mut current = wire;
        let mut visited = BTreeSet::new();
        loop {
            let source = index.node(&current.from_node)?;
            if CONDITIONAL_KINDS.contains(&source.kind.as_str()) {
                return Some((&current.from_node, current.from_socket.as_str()));
            }
            if !visited.insert(&current.from_node) {
                return None;
            }
            match self.incoming_flow(index, &current.from_node).as_slice() {
                [only] => current = only,
                _ => return None,
            }
        }
    }

    fn check_exclusive_joins(&self, index: &GraphIndex<'_>, errors: &mut Vec<ValidationError>) {
        for join in index.graph.nodes.iter().filter(|node| node.kind == WAIT_FOR_ALL_KIND) {
            let mut guarded: BTreeMap<&NodeId, Vec<(&str, &WireId)>> = BTreeMap::new();
            for wire in self.incoming_flow(index, &join.id) {
                if let Some((source, socket)) = self.conditional_guard(index, wire) {
                    guarded.entry(source).or_default().push((socket, &wire.id));
                }
            }
            for (source, arrivals) in guarded {
                let sockets: BTreeSet<&str> = arrivals.iter().map(|(socket, _)| *socket).collect();
                if sockets.len() > 1 {
                    errors.push(ValidationError::ExclusiveJoin {
                        node: join.id.clone(),
                        conditional: source.clone(),
                        wires: arrivals.into_iter().map(|(_, wire)| wire.clone()).collect(),
                    });
                }
            }
        }
    }

    fn check_required_inputs(&self, index: &GraphIndex<'_>, errors: &mut Vec<ValidationError>) {
        for node in &index.graph.nodes {
            let Some(spec) = self.specs.get(&node.kind) else {
                continue;
            };
            for input in spec.inputs.iter().filter(|socket| socket.required && !socket.ty.is_flow() && socket.default.is_none()) {
                let wired = index.wires_into(&node.id).iter().any(|wire| wire.to_socket == input.name);
                let configured = node.config.get(&input.name).is_some_and(|value| !value.is_null());
                if !wired && !configured {
                    errors.push(ValidationError::MissingRequiredInput { node: node.id.clone(), socket: input.name.clone() });
                }
            }
        }
    }
}

fn check_subgraph_calls(index: &GraphIndex<'_>, errors: &mut Vec<ValidationError>) {
    let mut targeted: BTreeSet<&NodeId> = BTreeSet::new();
    for reference in &index.graph.subgraphs {
        if !index.node(&reference.node_id).is_some_and(|node| node.kind == SUBGRAPH_KIND) {
            errors.push(ValidationError::OrphanSubgraph { node: reference.node_id.clone(), spell: reference.spell_id });
            continue;
        }
        if !targeted.insert(&reference.node_id) {
            errors.push(ValidationError::DuplicateSubgraphTarget(reference.node_id.clone()));
        }
    }
    for call in index.graph.nodes.iter().filter(|node| node.kind == SUBGRAPH_KIND) {
        let Some(reference) = index.graph.subgraphs.iter().find(|reference| reference.node_id == call.id) else {
            errors.push(ValidationError::SubgraphTargetMissing { node: call.id.clone() });
            continue;
        };
        let configured = call.config.get("spell_id").and_then(serde_json::Value::as_str).filter(|configured| !configured.is_empty());
        if let Some(configured) = configured.filter(|configured| *configured != reference.spell_id.0.to_string()) {
            errors.push(ValidationError::SubgraphTargetConflict { node: call.id.clone(), configured: configured.to_owned(), declared: reference.spell_id });
        }
    }
}

fn check_variables(graph: &SpellGraph, errors: &mut Vec<ValidationError>) {
    let mut names = BTreeSet::new();
    for variable in &graph.variables {
        if !names.insert(variable.name.as_str()) {
            errors.push(ValidationError::DuplicateVariable(variable.name.clone()));
        }
    }
}

fn find_cycles<'g>(graph: &'g SpellGraph, edges: &BTreeMap<&'g NodeId, Vec<&'g NodeId>>) -> Vec<Vec<NodeId>> {
    let mut marks: BTreeMap<&NodeId, Visit> = BTreeMap::new();
    let mut path: Vec<&NodeId> = Vec::new();
    let mut frames: Vec<(&NodeId, usize)> = Vec::new();
    let mut cycles = Vec::new();
    for root in &graph.nodes {
        if marks.contains_key(&root.id) {
            continue;
        }
        marks.insert(&root.id, Visit::InProgress);
        path.push(&root.id);
        frames.push((&root.id, 0));
        while let Some(frame) = frames.last_mut() {
            let (node, cursor) = *frame;
            let Some(next) = edges.get(node).and_then(|targets| targets.get(cursor)).copied() else {
                frames.pop();
                path.pop();
                marks.insert(node, Visit::Done);
                continue;
            };
            frame.1 += 1;
            match marks.get(next).copied() {
                Some(Visit::InProgress) => {
                    if let Some(start) = path.iter().position(|candidate| *candidate == next) {
                        cycles.push(path[start..].iter().map(|member| (*member).clone()).collect());
                    }
                }
                Some(Visit::Done) => {}
                None => {
                    marks.insert(next, Visit::InProgress);
                    path.push(next);
                    frames.push((next, 0));
                }
            }
        }
    }
    cycles
}
