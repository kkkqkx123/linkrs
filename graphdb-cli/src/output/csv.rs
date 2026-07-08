use crate::client::QueryResult;

pub fn format_csv(result: &QueryResult, null_string: &str) -> String {
    let mut output = String::new();

    output.push_str(&result.columns.join(","));
    output.push('\n');

    for row in &result.rows {
        let values: Vec<String> = result
            .columns
            .iter()
            .map(|col| {
                let val = row.get(col);
                match val {
                    Some(serde_json::Value::Null) | None => null_string.to_string(),
                    Some(serde_json::Value::String(s)) => {
                        if s.contains(',') || s.contains('"') || s.contains('\n') {
                            format!("\"{}\"", s.replace('"', "\"\""))
                        } else {
                            s.clone()
                        }
                    }
                    Some(v) => v.to_string(),
                }
            })
            .collect();
        output.push_str(&values.join(","));
        output.push('\n');
    }

    output
}
