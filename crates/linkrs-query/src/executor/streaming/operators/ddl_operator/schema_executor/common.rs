use linkrs_core::types::edge::EdgeTypeInfo;
use linkrs_core::Value;


/// Describe rows for an edge type's endpoint constraints.
///
/// The two leading `DESC EDGE` rows expose the current `src_tag` /
/// `dst_tag` constraint so `ALTER EDGE ... ADD/DROP FROM` changes are
/// directly observable. An empty constraint renders as `(unconstrained)`.
pub(super) fn endpoint_rows(edge_type: &EdgeTypeInfo) -> Vec<Vec<Value>> {
    let constraint = |tag: &str| {
        if tag.is_empty() {
            "(unconstrained)".to_string()
        } else {
            tag.to_string()
        }
    };
    vec![
        vec![
            Value::string("src_tag"),
            Value::string("TAG"),
            Value::Bool(false),
            Value::string(constraint(&edge_type.src_tag_name)),
            Value::string("edge endpoint constraint"),
        ],
        vec![
            Value::string("dst_tag"),
            Value::string("TAG"),
            Value::Bool(false),
            Value::string(constraint(&edge_type.dst_tag_name)),
            Value::string("edge endpoint constraint"),
        ],
    ]
}

pub(super) fn parse_vid_type_str(s: &str) -> Result<linkrs_core::types::DataType, String> {
    let upper = s.trim().to_uppercase();
    if upper == "INT64" {
        Ok(linkrs_core::types::DataType::BigInt)
    } else if upper == "INT32" {
        Ok(linkrs_core::types::DataType::Int)
    } else if upper == "INT16" || upper == "INT8" {
        Ok(linkrs_core::types::DataType::SmallInt)
    } else if upper == "STRING" {
        Ok(linkrs_core::types::DataType::String)
    } else if upper == "VID" {
        Err("VID is not a valid vertex ID type; use INT64, INT32, INT16, STRING or FIXED_STRING instead".to_string())
    } else if upper.starts_with("FIXED_STRING(") || upper.starts_with("FIXEDSTRING(") {
        let inner = upper
            .trim_start_matches("FIXED_STRING(")
            .trim_start_matches("FIXEDSTRING(")
            .trim_end_matches(')');
        if let Ok(n) = inner.parse::<usize>() {
            Ok(linkrs_core::types::DataType::FixedString(n))
        } else {
            Ok(linkrs_core::types::DataType::FixedString(32))
        }
    } else {
        Ok(linkrs_core::types::DataType::String)
    }
}
