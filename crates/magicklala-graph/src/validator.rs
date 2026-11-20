use std::collections::{BTreeMap, BTreeSet};

use crate::{NodeSpec, SPELL_SCHEMA_VERSION, SpellGraph, ValidationError};

pub struct SpellValidator {
    specs: BTreeMap<String, NodeSpec>,
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
        let mut seen = BTreeSet::new();
        for node in &graph.nodes {
            if !seen.insert(&node.id) {
                errors.push(ValidationError::DuplicateNode(node.id.clone()));
            }
            if !self.specs.contains_key(&node.kind) {
                errors.push(ValidationError::UnknownNodeKind { node: node.id.clone(), kind: node.kind.clone() });
            }
        }
        let mut wires = BTreeSet::new();
        for wire in &graph.wires {
            if !wires.insert(&wire.id) {
                errors.push(ValidationError::DuplicateWire(wire.id.clone()));
            }
            for endpoint in [&wire.from_node, &wire.to_node] {
                if graph.node(endpoint).is_none() {
                    errors.push(ValidationError::MissingNode { wire: wire.id.clone(), node: endpoint.clone() });
                }
            }
        }
        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }
}
