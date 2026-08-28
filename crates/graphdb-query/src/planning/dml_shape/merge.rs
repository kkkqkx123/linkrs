//! MERGE statement shape renderer.
//!
//! Only node/path patterns are supported (the shapes the parser produces for
//! `MERGE`). Non-outbound edge directions, edge ranges, and pattern
//! predicates are not re-rendered and fall back to the non-cached path.

use graphdb_core::types::EdgeDirection;
use graphdb_core::Value;
use crate::parser::ast::{EdgePattern, MergeStmt, NodePattern, PathElement, Pattern};

use super::render_contextual;

/// Render a canonical MERGE template, appending literal values.
pub(crate) fn render_merge(merge: &MergeStmt, values: &mut Vec<Value>) -> Option<String> {
    let mut out = String::from("MERGE ");
    render_pattern(&mut out, values, &merge.pattern)?;
    if let Some(on_create) = &merge.on_create {
        out.push_str(" ON CREATE SET ");
        super::update::render_set_assignments(&mut out, values, &on_create.assignments)?;
    }
    if let Some(on_match) = &merge.on_match {
        out.push_str(" ON MATCH SET ");
        super::update::render_set_assignments(&mut out, values, &on_match.assignments)?;
    }
    Some(out)
}

fn render_pattern(out: &mut String, values: &mut Vec<Value>, pattern: &Pattern) -> Option<()> {
    match pattern {
        Pattern::Node(node) => render_node(out, values, node),
        Pattern::Edge(edge) => render_edge(out, values, edge),
        Pattern::Path(path) => {
            for element in &path.elements {
                match element {
                    PathElement::Node(node) => render_node(out, values, node)?,
                    PathElement::Edge(edge) => render_edge(out, values, edge)?,
                    PathElement::Alternative(_)
                    | PathElement::Optional(_)
                    | PathElement::Repeated(_, _) => return None,
                }
            }
            Some(())
        }
        Pattern::Variable(_) => None,
    }
}

fn render_node(out: &mut String, values: &mut Vec<Value>, node: &NodePattern) -> Option<()> {
    if !node.predicates.is_empty() {
        return None;
    }
    out.push('(');
    if let Some(variable) = &node.variable {
        out.push_str(variable);
    }
    for label in &node.labels {
        out.push(':');
        out.push_str(label);
    }
    if let Some(properties) = &node.properties {
        out.push(' ');
        render_contextual(out, values, properties)?;
    }
    out.push(')');
    Some(())
}

fn render_edge(out: &mut String, values: &mut Vec<Value>, edge: &EdgePattern) -> Option<()> {
    if !edge.predicates.is_empty() || edge.range.is_some() {
        return None;
    }
    match edge.direction {
        EdgeDirection::Out => {
            out.push_str("-[");
        }
        EdgeDirection::In | EdgeDirection::Both => return None,
    }
    if let Some(variable) = &edge.variable {
        out.push_str(variable);
    }
    if !edge.edge_types.is_empty() {
        out.push(':');
        out.push_str(&edge.edge_types.join("|"));
    }
    if let Some(properties) = &edge.properties {
        out.push(' ');
        render_contextual(out, values, properties)?;
    }
    out.push_str("]->");
    Some(())
}
