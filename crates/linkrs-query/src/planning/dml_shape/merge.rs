//! MERGE statement shape renderer.

use crate::parser::ast::MergeStmt;
use linkrs_core::Value;

use super::pattern::render_pattern;
use super::update::render_set_assignments;

/// Render a canonical MERGE template, appending literal values.
pub(crate) fn render_merge(merge: &MergeStmt, values: &mut Vec<Value>) -> Option<String> {
    let mut out = String::from("MERGE ");
    render_pattern(&mut out, values, &merge.pattern)?;
    if let Some(on_create) = &merge.on_create {
        out.push_str(" ON CREATE SET ");
        render_set_assignments(&mut out, values, &on_create.assignments)?;
    }
    if let Some(on_match) = &merge.on_match {
        out.push_str(" ON MATCH SET ");
        render_set_assignments(&mut out, values, &on_match.assignments)?;
    }
    Some(out)
}
