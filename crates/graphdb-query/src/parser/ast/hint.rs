//! Join hint AST for `USING JOIN` clauses.
//!
//! A hint pins the join shape for the MATCH patterns that precede it:
//! ```text
//! MATCH (a)-[e1]->(b), (a)-[e2]->(c) USING JOIN BINARY(e1, e2) RETURN ...
//! MATCH (a)-[e1]->(b), (a)-[e2]->(c), (b)-[e3]->(c)
//!   USING JOIN MULTIWAY(e1, e2, e3) RETURN ...
//! ```
//! In `MULTIWAY` form the first variable is the probe side and the rest
//! are build sides. Hints name pattern variables; resolution against the
//! query graph happens in planning (`JoinHint::from_ast`), so an
//! unresolvable hint falls back to automatic planning instead of failing.

/// Parsed `USING JOIN` hint over pattern variables.
#[derive(Debug, Clone, PartialEq)]
pub enum JoinHintAst {
    /// Binary hash join of two sub-hints (leaf scans for now).
    Binary { left: String, right: String },
    /// Multi-way (WCO) join: first variable probes, the rest build.
    Multiway { probe: String, builds: Vec<String> },
}

impl JoinHintAst {
    /// Variables referenced by the hint, in order.
    pub fn variables(&self) -> Vec<&str> {
        match self {
            JoinHintAst::Binary { left, right } => vec![left.as_str(), right.as_str()],
            JoinHintAst::Multiway { probe, builds } => {
                let mut vars = Vec::with_capacity(builds.len() + 1);
                vars.push(probe.as_str());
                vars.extend(builds.iter().map(String::as_str));
                vars
            }
        }
    }
}
