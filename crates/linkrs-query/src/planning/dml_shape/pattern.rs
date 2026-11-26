//! Canonical rendering of data-writing graph patterns.

use crate::parser::ast::pattern::{
    CreateEdgePattern, EdgePattern, NodePattern, PathElement, Pattern,
};
use linkrs_core::types::EdgeDirection;
use linkrs_core::Value;

use super::render_contextual;

pub(crate) fn render_pattern(
    out: &mut String,
    values: &mut Vec<Value>,
    pattern: &Pattern,
) -> Option<()> {
    match pattern {
        Pattern::Node(node) => render_node(out, values, node),
        Pattern::Edge(edge) => render_edge(out, values, edge),
        Pattern::Path(path) => {
            for element in &path.elements {
                match element {
                    PathElement::Node(node) => render_node(out, values, node)?,
                    PathElement::Edge(edge) => render_edge(out, values, edge)?,
                    PathElement::CreateEdge(edge) => render_create_edge(out, values, edge)?,
                    PathElement::Alternative(_)
                    | PathElement::Optional(_)
                    | PathElement::Repeated(_, _)
                    | PathElement::Recursive(_) => return None,
                }
            }
            Some(())
        }
        Pattern::Variable(_) => None,
    }
}

pub(crate) fn render_node(
    out: &mut String,
    values: &mut Vec<Value>,
    node: &NodePattern,
) -> Option<()> {
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
    let suffix = match edge.direction {
        EdgeDirection::Out => {
            out.push_str("-[");
            "]->"
        }
        EdgeDirection::In => {
            out.push_str("<-[");
            "]-"
        }
        EdgeDirection::Both => {
            out.push_str("-[");
            "]-"
        }
    };
    render_edge_body(
        out,
        values,
        &edge.variable,
        &edge.edge_types,
        edge.properties.as_ref(),
    )?;
    out.push_str(suffix);
    Some(())
}

fn render_create_edge(
    out: &mut String,
    values: &mut Vec<Value>,
    edge: &CreateEdgePattern,
) -> Option<()> {
    let suffix = match edge.direction {
        EdgeDirection::Out => {
            out.push_str("-[");
            "]->"
        }
        EdgeDirection::In => {
            out.push_str("<-[");
            "]-"
        }
        EdgeDirection::Both => {
            out.push_str("-[");
            "]-"
        }
    };
    render_edge_body(
        out,
        values,
        &edge.variable,
        &edge.edge_types,
        edge.properties.as_ref(),
    )?;
    out.push_str(suffix);
    Some(())
}

fn render_edge_body(
    out: &mut String,
    values: &mut Vec<Value>,
    variable: &Option<String>,
    edge_types: &[String],
    properties: Option<&linkrs_core::types::expr::contextual::ContextualExpression>,
) -> Option<()> {
    if let Some(variable) = variable {
        out.push_str(variable);
    }
    if !edge_types.is_empty() {
        out.push(':');
        out.push_str(&edge_types.join("|"));
    }
    if let Some(properties) = properties {
        out.push(' ');
        render_contextual(out, values, properties)?;
    }
    Some(())
}
