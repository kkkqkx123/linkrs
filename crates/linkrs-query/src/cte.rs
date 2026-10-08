//! Common table expression (CTE) name mangling.
//!
//! A recursive-step pattern may scan the working table of an enclosing
//! `WITH RECURSIVE` CTE by naming the CTE as a node label
//! (`MATCH (m:my_cte)-[:KNOWS]->(x)`). Bound CTE labels are mangled with
//! [`CTE_TAG_PREFIX`] so they can never collide with real tag names and so
//! the spec builder can route them to a [`CteScan`](crate::executor::streaming::operators::spec::SourceSpec::CteScan)
//! source instead of storage.

/// Prefix marking a bound tag name as a CTE working-table reference.
///
/// Real tag names cannot contain this prefix in practice (it is not a valid
/// identifier start for schema objects); the spec builder treats any scan
/// tag with this prefix as a CTE scan.
pub const CTE_TAG_PREFIX: &str = "__cte__";

/// Default cap on recursive-CTE fixpoint iterations.
///
/// Mirrors the set-operation nesting guard (`max_depth = 100`) so runaway
/// recursions fail fast instead of looping forever.
pub const DEFAULT_RECURSIVE_CTE_MAX_ITERATIONS: u64 = 100;

/// Mangle a user CTE name into a collision-free internal tag name.
pub fn mangle_cte_name(name: &str) -> String {
    format!("{CTE_TAG_PREFIX}{name}")
}

/// Strip [`CTE_TAG_PREFIX`] from a bound tag name, returning the user CTE
/// name when the tag denotes a CTE working-table reference.
pub fn strip_cte_tag(tag: &str) -> Option<&str> {
    tag.strip_prefix(CTE_TAG_PREFIX)
}

/// Whether a bound scan tag denotes a CTE working-table reference.
pub fn is_cte_tag(tag: &str) -> bool {
    tag.starts_with(CTE_TAG_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mangle_roundtrip() {
        let mangled = mangle_cte_name("r");
        assert!(is_cte_tag(&mangled));
        assert_eq!(strip_cte_tag(&mangled), Some("r"));
        assert!(!is_cte_tag("Person"));
        assert_eq!(strip_cte_tag("Person"), None);
    }
}
