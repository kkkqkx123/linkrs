use crate::client::QueryResult;

pub fn format_json(result: &QueryResult) -> String {
    let mut rows_out = Vec::new();

    for row in &result.rows {
        let mut obj = serde_json::Map::new();
        for col in &result.columns {
            let val = row.get(col).cloned().unwrap_or(serde_json::Value::Null);
            obj.insert(col.clone(), val);
        }
        rows_out.push(serde_json::Value::Object(obj));
    }

    let output = serde_json::Value::Array(rows_out);
    serde_json::to_string_pretty(&output).unwrap_or_else(|_| "[]".to_string())
}
