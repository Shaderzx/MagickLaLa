use std::collections::{BTreeMap, BTreeSet};

use crate::{NodeId, NodeSpec, SPELL_SCHEMA_VERSION, SpellGraph, ValidationError};

pub struct SpellValidator {
    specs: BTreeMap<String, NodeSpec>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Visit {
    InProgress,
    Done,
}

impl SpellValidator {
    pub fn new(specs: impl IntoIterator<Item = NodeSpec>) -> Self {
        Self { specs: specs.into_iter().map(|spec| (spec.kind.clone(), spec)).collect() }
    }

    pub fn spec(&self, kind: &str) -> Option<&NodeSpec> {
        self.specs.get(kind)
    }

    pub fn validate(&self, graph: &SpellGraph) -> Result<(), Vec<ValidationError>> {
        let mut errors = Vec::new();
        if graph.schema_version != SPELL_SCHEMA_VERSION {
            errors.push(ValidationError::SchemaVersion { found: graph.schema_version, expected: SPELL_SCHEMA_VERSION });
        }
        self.check_nodes(graph, &mut errors);
        check_variables(graph, &mut errors);
        let value_edges = self.check_wires(graph, &mut errors);
        self.check_required_inputs(graph, &mut errors);
        for cycle in find_cycles(graph, &value_edges) {
            errors.push(ValidationError::ValueCycle(cycle));
        }
        if !graph.nodes.iter().any(|node| self.specs.get(&node.kind).is_some_and(|spec| spec.is_trigger)) {
            errors.push(ValidationError::NoTrigger);
        }
        for subgraph in graph.subgraphs.iter().filter(|subgraph| subgraph.spell_id == graph.id) {
            errors.push(ValidationError::SubgraphCycle { node: subgraph.node_id.clone(), spell: subgraph.spell_id });
        }
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

    fn spec_of(&self, graph: &SpellGraph, node: &NodeId) -> Option<&NodeSpec> {
        graph.node(node).and_then(|instance| self.specs.get(&instance.kind))
    }

    fn check_wires<'g>(&self, graph: &'g SpellGraph, errors: &mut Vec<ValidationError>) -> BTreeMap<&'g NodeId, Vec<&'g NodeId>> {
        let mut wire_ids = BTreeSet::new();
        let mut value_inputs: BTreeMap<(&NodeId, &str), usize> = BTreeMap::new();
        let mut value_edges: BTreeMap<&NodeId, Vec<&NodeId>> = BTreeMap::new();
        for wire in &graph.wires {
            if !wire_ids.insert(&wire.id) {
                errors.push(ValidationError::DuplicateWire(wire.id.clone()));
            }
            let mut endpoints_exist = true;
            for endpoint in [&wire.from_node, &wire.to_node] {
                if graph.node(endpoint).is_none() {
                    errors.push(ValidationError::MissingNode { wire: wire.id.clone(), node: endpoint.clone() });
                    endpoints_exist = false;
                }
            }
            if !endpoints_exist {
                continue;
            }
            let (Some(from_spec), Some(to_spec)) = (self.spec_of(graph, &wire.from_node), self.spec_of(graph, &wire.to_node)) else {
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

    fn check_required_inputs(&self, graph: &SpellGraph, errors: &mut Vec<ValidationError>) {
        for node in &graph.nodes {
            let Some(spec) = self.specs.get(&node.kind) else {
                continue;
            };
            for input in spec.inputs.iter().filter(|socket| socket.required && !socket.ty.is_flow() && socket.default.is_none()) {
                let wired = graph.wires_into(&node.id).any(|wire| wire.to_socket == input.name);
                let configured = node.config.get(&input.name).is_some_and(|value| !value.is_null());
                if !wired && !configured {
                    errors.push(ValidationError::MissingRequiredInput { node: node.id.clone(), socket: input.name.clone() });
                }
            }
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

fn find_cycles(graph: &SpellGraph, edges: &BTreeMap<&NodeId, Vec<&NodeId>>) -> Vec<Vec<NodeId>> {
    let mut marks: BTreeMap<&NodeId, Visit> = BTreeMap::new();
    let mut stack = Vec::new();
    let mut cycles = Vec::new();
    for node in &graph.nodes {
        if !marks.contains_key(&node.id) {
            visit(&node.id, edges, &mut marks, &mut stack, &mut cycles);
        }
    }
    cycles
}

fn visit<'g>(
    node: &'g NodeId,
    edges: &BTreeMap<&'g NodeId, Vec<&'g NodeId>>,
    marks: &mut BTreeMap<&'g NodeId, Visit>,
    stack: &mut Vec<&'g NodeId>,
    cycles: &mut Vec<Vec<NodeId>>,
) {
    marks.insert(node, Visit::InProgress);
    stack.push(node);
    for next in edges.get(node).into_iter().flatten().copied() {
        match marks.get(next).copied() {
            Some(Visit::InProgress) => {
                if let Some(start) = stack.iter().position(|candidate| *candidate == next) {
                    cycles.push(stack[start..].iter().map(|member| (*member).clone()).collect());
                }
            }
            Some(Visit::Done) => {}
            None => visit(next, edges, marks, stack, cycles),
        }
    }
    stack.pop();
    marks.insert(node, Visit::Done);
}
