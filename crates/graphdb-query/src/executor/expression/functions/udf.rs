//! Dynamic UDF support.
//!
//! The module provides the plugin interface (`plugin`), the error type
//! (`error`), the ownership wrapper keeping the library alive (`dynamic`),
//! and the file loading logic (`loader`).

pub mod dynamic;
pub mod error;
pub mod loader;
pub mod plugin;

pub use dynamic::{execute_isolated, DynamicPlugin};
pub use error::UdfError;
pub use loader::{LoadedPlugin, UdfLoader};
pub use plugin::{
    check_arity, SharedPlugin, UdfAbiVersionFn, UdfCreateFn, UdfDestroyFn, UdfPlugin,
    UDF_ABI_VERSION, UDF_ABI_VERSION_SYMBOL, UDF_CREATE_SYMBOL, UDF_DESTROY_SYMBOL,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::expression::{ExpressionError, ExpressionErrorType};
    use graphdb_core::Value;

    struct Double;

    impl UdfPlugin for Double {
        fn name(&self) -> &str {
            "test_double"
        }

        fn description(&self) -> &str {
            "doubles an integer"
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
                Value::Int(v) => Ok(Value::Int(v * 2)),
                other => Err(ExpressionError::new(
                    ExpressionErrorType::TypeError,
                    format!("expected int, got {other:?}"),
                )),
            }
        }
    }

    struct Panic;

    impl UdfPlugin for Panic {
        fn name(&self) -> &str {
            "test_panic"
        }

        fn description(&self) -> &str {
            "always panics"
        }

        fn min_arity(&self) -> usize {
            0
        }

        fn max_arity(&self) -> usize {
            0
        }

        fn is_pure(&self) -> bool {
            false
        }

        fn execute(&self, _args: &[Value]) -> Result<Value, ExpressionError> {
            panic!("boom");
        }
    }

    #[test]
    fn test_isolated_execution_success() {
        let plugin = Double;
        let result = execute_isolated(&plugin, &[Value::Int(21)]).expect("execution failed");
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn test_isolated_execution_arity_checked() {
        let plugin = Double;
        let err = execute_isolated(&plugin, &[]).unwrap_err();
        assert_eq!(err.error_type, ExpressionErrorType::InvalidArgumentCount);
    }

    #[test]
    fn test_isolated_execution_panic_contained() {
        let plugin = Panic;
        let err = execute_isolated(&plugin, &[]).unwrap_err();
        assert_eq!(err.error_type, ExpressionErrorType::FunctionExecutionError);
        assert!(err.message.contains("test_panic"));
    }

    #[test]
    fn test_loader_rejects_missing_file() {
        let err = UdfLoader::load(std::path::Path::new("/nonexistent/udf_plugin.so")).unwrap_err();
        assert!(matches!(err, UdfError::InvalidPath(_, _)));
    }

    #[test]
    fn test_loader_rejects_wrong_extension() {
        let dir = std::env::temp_dir();
        let path = dir.join("linkrs_udf_probe.txt");
        std::fs::write(&path, b"not a library").expect("write probe file");
        let err = UdfLoader::load(&path).unwrap_err();
        std::fs::remove_file(&path).ok();
        assert!(matches!(err, UdfError::InvalidPath(_, _)));
    }

    #[test]
    fn test_install_from_source_rejects_remote() {
        let err = UdfLoader::install_from_source("https://example.com/udf.so").unwrap_err();
        assert!(matches!(err, UdfError::RepoDownloadUnsupported(_)));
    }
}
