//! SET / REMOVE statement shape renderers.

use graphdb_core::Value;
use crate::parser::ast::{RemoveStmt, SetStmt};

use super::render_contextual;

/// Render a canonical SET template, appending literal values.
pub(crate) fn render_set(set: &SetStmt, values: &mut Vec<Value>) -> Option<String> {
    let mut out = String::from("SET ");
    super::update::render_set_assignments(&mut out, values, &set.assignments)?;
    Some(out)
}

/// Render a canonical REMOVE template, appending literal values.
pub(crate) fn render_remove(remove: &RemoveStmt, values: &mut Vec<Value>) -> Option<String> {
    let mut out = String::from("REMOVE ");
    for (index, item) in remove.items.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        render_contextual(&mut out, values, item)?;
    }
    Some(out)
}
