use std::path::Path;
use std::sync::Arc;

use graphdb_core::Value;

use super::super::udf::{SharedPlugin, UdfError, UdfPlugin};
use super::*;
use crate::executor::expression::{ExpressionError, ExpressionErrorType};

struct AddOne;

impl UdfPlugin for AddOne {
    fn name(&self) -> &str {
        "test_add_one"
    }

    fn description(&self) -> &str {
        "adds one"
    }

    fn min_arity(&self) -> usize {
        1
    }

    fn max_arity(&self) -> usize {
        1
    }

    fn is_pure(&self) -> bool {
        true
    }

    fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        match &args[0] {
            Value::Int(v) => Ok(Value::Int(v + 1)),
            other => Err(ExpressionError::new(
                ExpressionErrorType::TypeError,
                format!("expected int, got {other:?}"),
            )),
        }
    }
}

fn plugin() -> SharedPlugin {
    Arc::new(AddOne)
}

#[test]
fn test_register_and_execute_dynamic_plugin() {
    let mut registry = FunctionRegistry::new();
    let name = registry
        .register_dynamic_plugin(plugin())
        .expect("registration failed");
    assert_eq!(name, "test_add_one");
    assert!(registry.is_dynamic("TEST_ADD_ONE"));
    let result = registry
        .execute("test_add_one", &[Value::Int(41)])
        .expect("execution failed");
    assert_eq!(result, Value::Int(42));
    let infos = registry.list_dynamic_udfs();
    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0].name, "test_add_one");
}

#[test]
fn test_dynamic_plugin_duplicate_rejected() {
    let mut registry = FunctionRegistry::new();
    registry
        .register_dynamic_plugin(plugin())
        .expect("first registration failed");
    let err = registry.register_dynamic_plugin(plugin()).unwrap_err();
    assert!(matches!(err, UdfError::AlreadyLoaded(_, _)));
}

#[test]
fn test_dynamic_plugin_builtin_conflict_rejected() {
    let mut registry = FunctionRegistry::new();
    struct ShadowAbs;
    impl UdfPlugin for ShadowAbs {
        fn name(&self) -> &str {
            "abs"
        }
        fn description(&self) -> &str {
            "shadow"
        }
        fn min_arity(&self) -> usize {
            1
        }
        fn max_arity(&self) -> usize {
            1
        }
        fn is_pure(&self) -> bool {
            true
        }
        fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError> {
            Ok(args[0].clone())
        }
    }
    let err = registry
        .register_dynamic_plugin(Arc::new(ShadowAbs))
        .unwrap_err();
    assert!(matches!(err, UdfError::BuiltinConflict(_)));
}

#[test]
fn test_unload_dynamic_plugin() {
    let mut registry = FunctionRegistry::new();
    registry
        .register_dynamic_plugin(plugin())
        .expect("registration failed");
    registry
        .unload_dynamic_udf("test_add_one")
        .expect("unload failed");
    assert!(!registry.is_dynamic("test_add_one"));
    let err = registry
        .execute("test_add_one", &[Value::Int(1)])
        .unwrap_err();
    assert_eq!(err.error_type, ExpressionErrorType::UndefinedFunction);
    let err = registry.unload_dynamic_udf("test_add_one").unwrap_err();
    assert!(matches!(err, UdfError::NotLoaded(_)));
}

#[test]
fn test_reload_in_process_plugin_skipped() {
    let mut registry = FunctionRegistry::new();
    registry
        .register_dynamic_plugin(plugin())
        .expect("registration failed");
    let reloaded = registry
        .reload_dynamic_udf("test_add_one")
        .expect("reload failed");
    assert!(!reloaded);
}

#[test]
fn test_load_dynamic_udf_missing_file() {
    let mut registry = FunctionRegistry::new();
    let err = registry
        .load_dynamic_udf(Path::new("/nonexistent/udf_plugin.so"))
        .unwrap_err();
    assert!(matches!(err, UdfError::InvalidPath(_, _)));
}

#[test]
fn test_install_dynamic_udf_rejects_remote_source() {
    let mut registry = FunctionRegistry::new();
    let err = registry
        .install_dynamic_udf("https://example.com/udf.so")
        .unwrap_err();
    assert!(matches!(err, UdfError::RepoDownloadUnsupported(_)));
}
