//! Transaction-aware session variable store shared by the server and
//! embedded session implementations.

use parking_lot::RwLock;
use std::collections::HashMap;

use crate::core::Value;

/// A session-variable operation recorded while an explicit transaction is
/// active. The overlay guarantees that ROLLBACK (and ROLLBACK TO SAVEPOINT)
/// restores variables to their pre-statement values, matching transaction
/// semantics. `Set` records both the previous value (for restore) and the
/// new value (for snapshot merge); `Savepoint` marks a rollback boundary.
#[derive(Debug, Clone)]
enum VariableOp {
    Set {
        name: String,
        prev: Option<Value>,
        value: Value,
    },
    Savepoint {
        name: String,
    },
}

/// Session-scoped user variables (`$name`).
///
/// A base store plus a transaction overlay: assignments made while an
/// explicit transaction is active are recorded as operations instead of
/// mutating the base store directly. COMMIT merges the overlay into the
/// base store; ROLLBACK (and ROLLBACK TO SAVEPOINT) restores the base to
/// its pre-transaction values.
#[derive(Debug, Default)]
pub struct SessionVariables {
    base: RwLock<HashMap<String, Value>>,
    ops: RwLock<Vec<VariableOp>>,
}

impl SessionVariables {
    pub fn new() -> Self {
        Self::default()
    }

    /// Assign a session variable. When `in_transaction` is true, the
    /// assignment is recorded on the overlay instead of the base store so
    /// ROLLBACK / ROLLBACK TO SAVEPOINT restore the previous value.
    pub fn set_variable(&self, name: String, value: Value, in_transaction: bool) {
        if in_transaction {
            let prev = self.variable_value(&name);
            self.ops
                .write()
                .push(VariableOp::Set { name, prev, value });
        } else {
            self.base.write().insert(name, value);
        }
    }

    /// Effective value of a session variable: transaction overlay first,
    /// then the base store.
    pub fn variable_value(&self, name: &str) -> Option<Value> {
        let ops = self.ops.read();
        for op in ops.iter().rev() {
            if let VariableOp::Set {
                name: op_name,
                value,
                ..
            } = op
            {
                if op_name == name {
                    return Some(value.clone());
                }
            }
        }
        self.base.read().get(name).cloned()
    }

    /// Snapshot of all session variables (base + overlay) for injection as
    /// query inputs.
    pub fn variables_snapshot(&self) -> HashMap<String, Value> {
        let mut merged = self.base.read().clone();
        for op in self.ops.read().iter() {
            if let VariableOp::Set { name, value, .. } = op {
                merged.insert(name.clone(), value.clone());
            }
        }
        merged
    }

    /// COMMIT: apply overlay operations to the base store and clear the
    /// overlay (the last assignment of each variable wins).
    pub fn commit_variables(&self) {
        let mut base = self.base.write();
        let ops = std::mem::take(&mut *self.ops.write());
        for op in ops {
            if let VariableOp::Set { name, value, .. } = op {
                base.insert(name, value);
            }
        }
    }

    /// Full ROLLBACK: restore pre-transaction values and clear the overlay.
    pub fn rollback_variables(&self) {
        self.restore_overlay_after(None);
    }

    /// ROLLBACK TO SAVEPOINT: restore values assigned after the named
    /// savepoint and truncate the overlay at the savepoint marker.
    pub fn rollback_variables_to(&self, savepoint_name: &str) -> bool {
        self.restore_overlay_after(Some(savepoint_name))
    }

    /// RELEASE SAVEPOINT: drop the marker (operations stay part of the
    /// transaction; they are no longer individually rollback-able).
    pub fn release_variable_savepoint(&self, savepoint_name: &str) {
        let mut ops = self.ops.write();
        ops.retain(|op| !matches!(op, VariableOp::Savepoint { name } if name == savepoint_name));
    }

    /// SAVEPOINT: record a variable-overlay boundary so ROLLBACK TO can
    /// restore assignments made after the savepoint.
    pub fn push_variable_savepoint(&self, savepoint_name: &str) {
        self.ops
            .write()
            .push(VariableOp::Savepoint { name: savepoint_name.to_string() });
    }

    /// Restore variable values for operations at or after `savepoint_name`
    /// (or the whole overlay when `None`), then truncate those operations.
    /// Returns whether a matching savepoint marker was found.
    fn restore_overlay_after(&self, savepoint_name: Option<&str>) -> bool {
        let mut base = self.base.write();
        let mut ops = self.ops.write();
        let removed: Vec<VariableOp> = match savepoint_name {
            Some(name) => {
                let mut found = None;
                for (idx, op) in ops.iter().enumerate() {
                    if matches!(op, VariableOp::Savepoint { name: n } if n == name) {
                        found = Some(idx + 1);
                    }
                }
                match found {
                    Some(idx) => ops.drain(idx..).collect(),
                    None => return false,
                }
            }
            None => std::mem::take(&mut *ops),
        };
        for op in removed.iter().rev() {
            if let VariableOp::Set { name, prev, .. } = op {
                match prev {
                    Some(value) => {
                        base.insert(name.clone(), value.clone());
                    }
                    None => {
                        base.remove(name);
                    }
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> SessionVariables {
        SessionVariables::new()
    }

    #[test]
    fn set_outside_transaction_mutates_base() {
        let s = store();
        s.set_variable("x".to_string(), Value::Int(1), false);
        assert_eq!(s.variable_value("x"), Some(Value::Int(1)));
        assert_eq!(s.variables_snapshot().len(), 1);
    }

    #[test]
    fn rollback_restores_pre_transaction_value() {
        let s = store();
        s.set_variable("x".to_string(), Value::Int(1), false);
        s.set_variable("x".to_string(), Value::Int(2), true);
        s.set_variable("y".to_string(), Value::string("txn"), true);
        assert_eq!(s.variable_value("x"), Some(Value::Int(2)));
        assert_eq!(s.variable_value("y"), Some(Value::string("txn")));
        s.rollback_variables();
        assert_eq!(s.variable_value("x"), Some(Value::Int(1)));
        assert_eq!(s.variable_value("y"), None);
    }

    #[test]
    fn commit_merges_overlay() {
        let s = store();
        s.set_variable("a".to_string(), Value::Int(1), false);
        s.set_variable("a".to_string(), Value::Int(5), true);
        s.commit_variables();
        assert_eq!(s.variable_value("a"), Some(Value::Int(5)));
        assert_eq!(s.variables_snapshot().len(), 1);
    }

    #[test]
    fn rollback_to_savepoint_restores_after_marker() {
        let s = store();
        s.set_variable("a".to_string(), Value::Int(1), false);
        s.push_variable_savepoint("sp1");
        s.set_variable("a".to_string(), Value::Int(2), true);
        s.set_variable("b".to_string(), Value::Int(3), true);
        assert!(s.rollback_variables_to("sp1"));
        assert_eq!(s.variable_value("a"), Some(Value::Int(1)));
        assert_eq!(s.variable_value("b"), None);
        s.set_variable("b".to_string(), Value::Int(4), true);
        assert!(!s.rollback_variables_to("missing"));
        assert_eq!(s.variable_value("b"), Some(Value::Int(4)));
    }

    #[test]
    fn release_savepoint_drops_marker() {
        let s = store();
        s.push_variable_savepoint("sp2");
        s.release_variable_savepoint("sp2");
        assert!(!s.rollback_variables_to("sp2"));
    }

    #[test]
    fn rollback_of_new_variable_removes_it() {
        let s = store();
        s.set_variable("new".to_string(), Value::Int(42), true);
        s.rollback_variables();
        assert_eq!(s.variable_value("new"), None);
    }
}