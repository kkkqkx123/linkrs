use linkrs_core::types::edge::EdgeTypeInfo;
use linkrs_core::types::PropertyDef;
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
    } else if upper == "FLOAT" {
        Ok(linkrs_core::types::DataType::Float)
    } else if upper == "DOUBLE" {
        Ok(linkrs_core::types::DataType::Double)
    } else if upper == "VID" {
        Err("VID is not a valid vertex ID type; use INT64, INT32, INT16, STRING or FIXED_STRING instead".to_string())
    } else if upper.starts_with("FIXED_STRING(") || upper.starts_with("FIXEDSTRING(") {
        let inner = upper
            .trim_start_matches("FIXED_STRING(")
            .trim_start_matches("FIXEDSTRING(")
            .trim_end_matches(')');
        if !upper.ends_with(')') {
            return Err(format!(
                "Invalid vid_type '{}'; FIXED_STRING requires a length like FIXED_STRING(32)",
                s
            ));
        }
        match inner.parse::<usize>() {
            Ok(n) if n > 0 => Ok(linkrs_core::types::DataType::FixedString(n)),
            _ => Err(format!(
                "Invalid vid_type '{}'; FIXED_STRING length must be a positive integer",
                s
            )),
        }
    } else {
        Err(format!(
            "Unknown vid_type '{}'; supported vid types: INT64, INT32, INT16, STRING, FIXED_STRING(n)",
            s
        ))
    }
}

/// Escape a string for embedding in a single-quoted SQL literal.
pub(super) fn escape_single_quoted(s: &str) -> String {
    s.replace('\'', "''")
}

/// Render a default value as a SQL literal so `SHOW CREATE` output can be
/// re-parsed. Strings are single-quoted; numbers and booleans are bare;
/// `NULL` renders as `NULL`.
pub(super) fn sql_literal(value: &Value) -> String {
    match value {
        Value::String(s) => format!("'{}'", escape_single_quoted(s)),
        Value::FixedString(s) => format!("'{}'", escape_single_quoted(s)),
        Value::Bool(b) => b.to_string(),
        Value::SmallInt(i) => i.to_string(),
        Value::Int(i) => i.to_string(),
        Value::BigInt(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Double(d) => d.to_string(),
        Value::Decimal128(d) => d.to_string(),
        Value::Null(_) | Value::Empty => "NULL".to_string(),
        Value::Date(d) => format!("'{:04}-{:02}-{:02}'", d.year, d.month, d.day),
        Value::Time(t) => format!(
            "'{:02}:{:02}:{:02}.{:06}'",
            t.hour, t.minute, t.sec, t.microsec
        ),
        Value::DateTime(dt) => format!(
            "'{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:06}'",
            dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.sec, dt.microsec
        ),
        other => format!("'{}'", escape_single_quoted(&format!("{other}"))),
    }
}

/// Render one property definition as DDL (`name TYPE [NOT NULL]
/// [DEFAULT <literal>] [COMMENT '<text>']`). `SERIAL` columns render with the
/// `SERIAL` sugar so the auto-increment flag survives a `SHOW CREATE` round
/// trip. `CHANGE` semantics are rename-only: this renderer emits the full
/// stored definition, while `ALTER ... CHANGE` intentionally does not restore
/// defaults/comments/nullability.
pub(super) fn render_property_def(prop: &PropertyDef) -> String {
    let mut out = String::new();
    out.push_str(&prop.name);
    out.push(' ');
    if prop.serial {
        out.push_str("SERIAL");
    } else {
        out.push_str(&prop.data_type.to_string());
        if !prop.nullable {
            out.push_str(" NOT NULL");
        }
    }
    if let Some(default) = &prop.default {
        out.push_str(" DEFAULT ");
        out.push_str(&sql_literal(default));
    }
    if let Some(comment) = &prop.comment {
        out.push_str(" COMMENT '");
        out.push_str(&escape_single_quoted(comment));
        out.push('\'');
    }
    out
}

/// Canonical `vid_type=...` spelling for `CREATE SPACE` round trips.
pub(super) fn vid_type_spelling(vid_type: &linkrs_core::types::DataType) -> String {
    use linkrs_core::types::DataType::*;
    match vid_type {
        BigInt => "INT64".to_string(),
        Int => "INT32".to_string(),
        SmallInt => "INT16".to_string(),
        String => "STRING".to_string(),
        FixedString(n) => format!("FIXED_STRING({})", n),
        Float => "FLOAT".to_string(),
        Double => "DOUBLE".to_string(),
        other => other.to_string(),
    }
}

pub(super) fn render_ttl_parts(ttl_duration: Option<i64>, ttl_col: Option<&str>) -> Vec<String> {
    match (ttl_duration, ttl_col) {
        (Some(duration), Some(col)) => vec![
            format!("ttl_duration={}", duration),
            format!("ttl_col={}", col),
        ],
        _ => Vec::new(),
    }
}

pub(super) fn render_create_tag(
    tag_name: &str,
    properties: &[PropertyDef],
    ttl_duration: Option<i64>,
    ttl_col: Option<&str>,
    comment: Option<&str>,
) -> String {
    let mut parts: Vec<String> = properties.iter().map(render_property_def).collect();
    parts.extend(render_ttl_parts(ttl_duration, ttl_col));
    if let Some(comment) = comment {
        parts.push(format!("comment='{}'", escape_single_quoted(comment)));
    }
    format!(
        "CREATE TAG IF NOT EXISTS {} ({})",
        tag_name,
        parts.join(", ")
    )
}

pub(super) fn render_create_edge(
    edge_name: &str,
    properties: &[PropertyDef],
    src_tag: &str,
    dst_tag: &str,
    ttl_duration: Option<i64>,
    ttl_col: Option<&str>,
    comment: Option<&str>,
) -> String {
    let mut parts: Vec<String> = properties.iter().map(render_property_def).collect();
    parts.extend(render_ttl_parts(ttl_duration, ttl_col));
    if let Some(comment) = comment {
        parts.push(format!("comment='{}'", escape_single_quoted(comment)));
    }
    let mut ddl = format!(
        "CREATE EDGE IF NOT EXISTS {} ({})",
        edge_name,
        parts.join(", ")
    );
    if !src_tag.is_empty() && !dst_tag.is_empty() {
        ddl.push_str(&format!(" FROM {} TO {}", src_tag, dst_tag));
    }
    ddl
}

pub(super) fn render_create_space(
    space_name: &str,
    vid_type: &linkrs_core::types::DataType,
    comment: Option<&str>,
) -> String {
    let mut ddl = format!(
        "CREATE SPACE {} (vid_type={}",
        space_name,
        vid_type_spelling(vid_type)
    );
    if let Some(comment) = comment {
        ddl.push_str(&format!(", comment='{}'", escape_single_quoted(comment)));
    }
    ddl.push(')');
    ddl
}

pub(super) fn render_create_index(
    tag_or_edge: &str,
    index_name: &str,
    target_name: &str,
    properties: &[String],
) -> String {
    format!(
        "CREATE {} INDEX {} ON {}({})",
        tag_or_edge,
        index_name,
        target_name,
        properties.join(", ")
    )
}
