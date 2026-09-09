//! User-defined macro expansion at bind time.
//!
//! A macro call `name(args...)` in expression position is expanded before
//! normal function binding: actual argument expressions are substituted for
//! the macro parameters in the stored body template, and the result is bound
//! recursively. Expansion is purely syntactic; the planner and executor never
//! see macros.

use std::collections::HashMap;
use std::sync::Arc;

use graphdb_core::error::{DBError, QueryError};
use graphdb_core::metadata::MacroManager;
use graphdb_core::types::expr::{substitute_variables, Expression, FunctionArg};
use graphdb_core::{DBResult, DataType};

use super::bind::Binder;
use super::bound::BoundExpression;

/// Maximum nested macro expansion depth (direct or indirect recursion).
pub const MAX_MACRO_EXPANSION_DEPTH: usize = 32;

impl Binder {
    /// Attach the shared macro catalog. When set, function calls that name a
    /// defined macro expand at bind time instead of resolving as functions.
    pub fn with_macro_manager(mut self, manager: Arc<MacroManager>) -> Self {
        self.macro_manager = Some(manager);
        self
    }

    /// Try to expand `name(args)` as a user-defined macro.
    ///
    /// Returns `Ok(None)` when no macro is defined under `name`, so the
    /// caller falls through to normal function binding.
    pub(crate) fn try_expand_macro(
        &mut self,
        name: &str,
        args: &[FunctionArg],
        type_hint: Option<&DataType>,
    ) -> DBResult<Option<BoundExpression>> {
        let Some(ref manager) = self.macro_manager else {
            return Ok(None);
        };
        let Some(def) = manager.get_macro(name) else {
            return Ok(None);
        };

        let canonical = name.to_ascii_uppercase();
        if self.expanding.iter().any(|n| n == &canonical) {
            return Err(DBError::from(QueryError::invalid_query(format!(
                "Recursive macro expansion detected for macro '{}'",
                def.name
            ))));
        }
        if self.expanding.len() >= MAX_MACRO_EXPANSION_DEPTH {
            return Err(DBError::from(QueryError::invalid_query(format!(
                "Macro expansion depth exceeds limit ({}) for macro '{}'",
                MAX_MACRO_EXPANSION_DEPTH, def.name
            ))));
        }

        // Map actual arguments onto parameters (positional first, then
        // named). Missing arguments fall back to parameter defaults.
        let mut mapping: HashMap<String, Expression> = HashMap::new();
        let mut assigned = vec![false; def.params.len()];
        let mut positional = 0usize;
        for arg in args {
            match arg {
                FunctionArg::Positional(expr) => {
                    while positional < def.params.len() && assigned[positional] {
                        positional += 1;
                    }
                    if positional >= def.params.len() {
                        return Err(DBError::from(QueryError::invalid_query(format!(
                            "Macro '{}' takes at most {} arguments ({} given)",
                            def.name,
                            def.params.len(),
                            args.len()
                        ))));
                    }
                    mapping.insert(def.params[positional].name.clone(), expr.clone());
                    assigned[positional] = true;
                    positional += 1;
                }
                FunctionArg::Named {
                    name: arg_name,
                    value,
                } => {
                    let Some(index) = def
                        .params
                        .iter()
                        .position(|p| p.name.eq_ignore_ascii_case(arg_name))
                    else {
                        return Err(DBError::from(QueryError::invalid_query(format!(
                            "Macro '{}' has no parameter named '{}'",
                            def.name, arg_name
                        ))));
                    };
                    if assigned[index] {
                        return Err(DBError::from(QueryError::invalid_query(format!(
                            "Duplicate argument for macro '{}' parameter '{}'",
                            def.name, def.params[index].name
                        ))));
                    }
                    mapping.insert(def.params[index].name.clone(), value.clone());
                    assigned[index] = true;
                }
            }
        }
        for (index, param) in def.params.iter().enumerate() {
            if !assigned[index] {
                match &param.default {
                    Some(default) => {
                        mapping.insert(param.name.clone(), default.clone());
                    }
                    None => {
                        return Err(DBError::from(QueryError::invalid_query(format!(
                            "Macro '{}' missing required argument '{}'",
                            def.name, param.name
                        ))));
                    }
                }
            }
        }

        let expanded = substitute_variables(&def.body, &mapping);
        self.expanding.push(canonical);
        let bound = self.bind_inner_expr(&expanded, type_hint);
        self.expanding.pop();
        bound.map(Some)
    }
}
