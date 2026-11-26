//! Data-form CREATE shape renderer.

use crate::parser::ast::stmt::{CreateStmt, CreateTarget};
use linkrs_core::types::EdgeDirection;
use linkrs_core::Value;

use super::pattern::render_pattern;
use super::render_contextual;

pub(crate) fn render_create(create: &CreateStmt, values: &mut Vec<Value>) -> Option<String> {
    if create.if_not_exists {
        return None;
    }
    let mut out = String::from("CREATE ");
    match &create.target {
        CreateTarget::Node {
            variable,
            labels,
            properties,
        } => {
            out.push('(');
            if let Some(variable) = variable {
                out.push_str(variable);
            }
            for label in labels {
                out.push(':');
                out.push_str(label);
            }
            if let Some(properties) = properties {
                out.push(' ');
                render_contextual(&mut out, values, properties)?;
            }
            out.push(')');
        }
        CreateTarget::Edge {
            variable,
            edge_type,
            src,
            dst,
            properties,
            direction,
        } => {
            out.push('(');
            render_contextual(&mut out, values, src)?;
            out.push(')');
            match direction {
                EdgeDirection::Out => out.push_str("-["),
                EdgeDirection::In => out.push_str("<-["),
                EdgeDirection::Both => out.push_str("-["),
            }
            if let Some(variable) = variable {
                out.push_str(variable);
            }
            out.push(':');
            out.push_str(edge_type);
            if let Some(properties) = properties {
                out.push(' ');
                render_contextual(&mut out, values, properties)?;
            }
            match direction {
                EdgeDirection::Out => out.push_str("]->("),
                EdgeDirection::In => out.push_str("]-("),
                EdgeDirection::Both => out.push_str("]-("),
            }
            render_contextual(&mut out, values, dst)?;
            out.push(')');
        }
        CreateTarget::Path { patterns } => {
            for (index, pattern) in patterns.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                render_pattern(&mut out, values, pattern)?;
            }
        }
        _ => return None,
    }
    Some(out)
}
