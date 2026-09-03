//! Function-name classification for plan analysis.
//!
//! Identifies functions whose arguments are evaluated per element of a
//! list (lambda-style evaluation). Plan passes that reason about
//! evaluation order (for example factorization flattening) consult this
//! module instead of hardcoding function names, so the classification
//! stays consistent wherever it is used.

/// Names evaluated per list element with the lambda at argument index 1.
///
/// `list_extract` takes a list and an index rather than a lambda, but it
/// has historically received the same per-element treatment; that
/// treatment is preserved here to avoid changing flattening behavior.
/// `list_any`, `list_all` and `list_single` are predicate variants that
/// evaluate the lambda per element like `list_filter`.
const LIST_LAMBDA_NAMES: [&str; 7] = [
    "list_filter",
    "list_extract",
    "list_transform",
    "list_reduce",
    "list_any",
    "list_all",
    "list_single",
];

/// Report whether a function evaluates an argument per list element.
///
/// Matching is case-insensitive to mirror binder normalization.
pub fn is_list_lambda(name: &str) -> bool {
    LIST_LAMBDA_NAMES.contains(&name.to_lowercase().as_str())
}

/// Argument position of the per-element expression for a list lambda.
///
/// Returns `None` for functions without per-element evaluation.
pub fn list_lambda_arg_index(name: &str) -> Option<usize> {
    if is_list_lambda(name) {
        Some(1)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_lambdas_match_case_insensitively() {
        assert!(is_list_lambda("list_filter"));
        assert!(is_list_lambda("LIST_TRANSFORM"));
        assert!(is_list_lambda("List_Extract"));
        assert!(is_list_lambda("list_reduce"));
        assert!(is_list_lambda("list_any"));
        assert!(is_list_lambda("LIST_ALL"));
        assert!(is_list_lambda("List_Single"));
    }

    #[test]
    fn eager_functions_do_not_match() {
        assert!(!is_list_lambda("list_append"));
        assert!(!is_list_lambda("list_contains"));
        assert!(!is_list_lambda("abs"));
        assert!(!is_list_lambda(""));
    }

    #[test]
    fn lambda_arg_index_is_second_position() {
        assert_eq!(list_lambda_arg_index("list_filter"), Some(1));
        assert_eq!(list_lambda_arg_index("list_transform"), Some(1));
        assert_eq!(list_lambda_arg_index("list_append"), None);
    }
}
