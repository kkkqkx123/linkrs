use crate::client::QueryResult;

fn value_to_string(val: &serde_json::Value, null_string: &str) -> String {
    match val {
        serde_json::Value::Null => null_string.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => {
            let items: Vec<String> = arr
                .iter()
                .map(|v| value_to_string(v, null_string))
                .collect();
            format!("[{}]", items.join(", "))
        }
        serde_json::Value::Object(obj) => {
            let items: Vec<String> = obj
                .iter()
                .map(|(k, v)| format!("{}: {}", k, value_to_string(v, null_string)))
                .collect();
            format!("{{{}}}", items.join(", "))
        }
    }
}

pub fn format_table(result: &QueryResult, null_string: &str) -> String {
    if result.columns.is_empty() && result.rows.is_empty() {
        return "(0 rows)".to_string();
    }

    if result.rows.is_empty() {
        let mut output = String::new();
        for col in &result.columns {
            if !output.is_empty() {
                output.push_str(" | ");
            }
            output.push_str(col);
        }
        output.push_str("\n(0 rows)");
        return output;
    }

    let mut builder = tabled::builder::Builder::default();

    builder.push_record(
        result
            .columns
            .iter()
            .map(|c| c.as_str())
            .collect::<Vec<_>>(),
    );

    for row in &result.rows {
        let values: Vec<String> = result
            .columns
            .iter()
            .map(|col| {
                row.get(col)
                    .map(|v| value_to_string(v, null_string))
                    .unwrap_or_else(|| null_string.to_string())
            })
            .collect();
        builder.push_record(&values);
    }

    let mut table = builder.build();
    table.with(tabled::settings::Style::rounded());

    format!("{}\n\n({} rows)", table, result.row_count)
}

pub fn format_vertical(result: &QueryResult, null_string: &str) -> String {
    if result.rows.is_empty() {
        return "(0 rows)".to_string();
    }

    let mut output = String::new();

    for (i, row) in result.rows.iter().enumerate() {
        output.push_str(&format!("-[ RECORD {} ]-\n", i + 1));
        for col in &result.columns {
            let value = row
                .get(col)
                .map(|v| value_to_string(v, null_string))
                .unwrap_or_else(|| null_string.to_string());
            output.push_str(&format!("{:20} | {}\n", col, value));
        }
    }

    output.push_str(&format!("\n({} rows)", result.row_count));
    output
}
