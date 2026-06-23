//! Metadata definition for aggregate functions
//! Define the supported aggregate functions and their constraints.

#[derive(Debug, Clone)]
pub struct AggFunctionMeta {
    pub name: &'static str,
    pub require_numeric: bool,
    pub allow_wildcard: bool,
}

impl AggFunctionMeta {
    pub fn get(name: &str) -> Option<Self> {
        match name.to_uppercase().as_str() {
            "COUNT" => Some(AggFunctionMeta {
                name: "COUNT",
                require_numeric: false,
                allow_wildcard: true,
            }),
            "SUM" => Some(AggFunctionMeta {
                name: "SUM",
                require_numeric: true,
                allow_wildcard: false,
            }),
            "AVG" => Some(AggFunctionMeta {
                name: "AVG",
                require_numeric: true,
                allow_wildcard: false,
            }),
            "MAX" => Some(AggFunctionMeta {
                name: "MAX",
                require_numeric: false,
                allow_wildcard: false,
            }),
            "MIN" => Some(AggFunctionMeta {
                name: "MIN",
                require_numeric: false,
                allow_wildcard: false,
            }),
            "STD" => Some(AggFunctionMeta {
                name: "STD",
                require_numeric: true,
                allow_wildcard: false,
            }),
            "BIT_AND" => Some(AggFunctionMeta {
                name: "BIT_AND",
                require_numeric: true,
                allow_wildcard: false,
            }),
            "BIT_OR" => Some(AggFunctionMeta {
                name: "BIT_OR",
                require_numeric: true,
                allow_wildcard: false,
            }),
            "BIT_XOR" => Some(AggFunctionMeta {
                name: "BIT_XOR",
                require_numeric: true,
                allow_wildcard: false,
            }),
            "COLLECT" => Some(AggFunctionMeta {
                name: "COLLECT",
                require_numeric: false,
                allow_wildcard: false,
            }),
            "COLLECT_SET" => Some(AggFunctionMeta {
                name: "COLLECT_SET",
                require_numeric: false,
                allow_wildcard: false,
            }),
            _ => None,
        }
    }

    pub fn all_functions() -> Vec<&'static str> {
        vec![
            "COUNT",
            "SUM",
            "AVG",
            "MAX",
            "MIN",
            "STD",
            "BIT_AND",
            "BIT_OR",
            "BIT_XOR",
            "COLLECT",
            "COLLECT_SET",
        ]
    }

    pub fn is_valid(name: &str) -> bool {
        Self::get(name).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agg_function_meta_get_count() {
        let meta = AggFunctionMeta::get("COUNT").expect("COUNT function should exist");
        assert_eq!(meta.name, "COUNT");
        assert!(!meta.require_numeric);
        assert!(meta.allow_wildcard);
    }

    #[test]
    fn test_agg_function_meta_get_sum() {
        let meta = AggFunctionMeta::get("SUM").expect("SUM function should exist");
        assert_eq!(meta.name, "SUM");
        assert!(meta.require_numeric);
        assert!(!meta.allow_wildcard);
    }

    #[test]
    fn test_agg_function_meta_get_case_insensitive() {
        assert!(AggFunctionMeta::get("COUNT").is_some());
        assert!(AggFunctionMeta::get("count").is_some());
        assert!(AggFunctionMeta::get("CoUnT").is_some());
    }

    #[test]
    fn test_agg_function_meta_get_invalid() {
        let meta = AggFunctionMeta::get("INVALID_FUNC");
        assert!(meta.is_none());
    }

    #[test]
    fn test_all_functions_count() {
        let funcs = AggFunctionMeta::all_functions();
        assert_eq!(funcs.len(), 11);
    }

    #[test]
    fn test_is_valid() {
        assert!(AggFunctionMeta::is_valid("COUNT"));
        assert!(AggFunctionMeta::is_valid("sum"));
        assert!(AggFunctionMeta::is_valid("COLLECT_SET"));
        assert!(!AggFunctionMeta::is_valid("UNKNOWN"));
    }
}
