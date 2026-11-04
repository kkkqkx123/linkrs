use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _};
use gpui_kit::{
    div, px, AnyElement, App, Div, FontWeight, IntoElement, ParentElement as _, SharedString,
    Styled as _,
};
use serde_json::Value;

use super::pill::{pill, Tone};
use super::table::{Cell, Column, SimpleTable};

const MAX_OBJECT_ROWS: usize = 24;
const MAX_TABLE_ROWS: usize = 50;
const MAX_TABLE_COLUMNS: usize = 8;
const MAX_VALUE_CHARS: usize = 240;

/// Render a payload as structure instead of a raw dump: objects become
/// key/value rows, arrays of objects become tables and arrays of scalars
/// become chips. Deeply nested values fall back to a bounded raw block.
pub fn render_json_block(value: &Value, cx: &App) -> AnyElement {
    let id = format!("json-{:x}", value as *const Value as usize);
    block(value, &id, 0, cx)
}

/// Render a payload as pretty-printed JSON inside a bordered code block.
pub fn render_raw_json(value: &Value, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let pretty = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    div()
        .w_full()
        .max_h(px(360.))
        .overflow_y_scrollbar()
        .p(px(12.))
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.table_row_border)
        .bg(theme.muted)
        .font_family(theme.mono_font_family.clone())
        .text_size(px(12.))
        .text_color(theme.foreground)
        .child(SharedString::from(pretty))
        .into_any_element()
}

fn block(value: &Value, id: &str, depth: usize, cx: &App) -> AnyElement {
    match value {
        Value::Object(map) if !map.is_empty() => object_block(map, id, depth, cx),
        Value::Array(items) if !items.is_empty() => array_block(items, None, id, depth, cx),
        Value::Null => muted("No data", cx),
        _ => {
            let theme = cx.theme();
            div()
                .text_size(px(13.))
                .text_color(theme.foreground)
                .child(SharedString::from(scalar(value)))
                .into_any_element()
        }
    }
}

fn object_block(
    map: &serde_json::Map<String, Value>,
    id: &str,
    depth: usize,
    cx: &App,
) -> AnyElement {
    let mut rows: Vec<(SharedString, Value)> = Vec::new();
    let mut nested: Vec<AnyElement> = Vec::new();

    for (key, value) in map.iter().take(MAX_OBJECT_ROWS) {
        match value {
            Value::Object(inner) if !inner.is_empty() && depth < 1 => {
                nested.push(section(key, object_block(inner, id, depth + 1, cx), cx));
            }
            Value::Array(items) if !items.is_empty() && depth < 2 => {
                nested.push(section(
                    key,
                    array_block(items, Some(key), id, depth + 1, cx),
                    cx,
                ));
            }
            _ => rows.push((SharedString::from(key.clone()), value.clone())),
        }
    }

    v_flex()
        .w_full()
        .gap_3()
        .children((!rows.is_empty()).then(|| {
            v_flex()
                .w_full()
                .children(rows.into_iter().map(|(key, value)| kv_row(key, &value, cx)))
                .into_any_element()
        }))
        .children(nested)
        .into_any_element()
}

fn array_block(items: &[Value], key: Option<&str>, id: &str, depth: usize, cx: &App) -> AnyElement {
    if items.iter().all(Value::is_object) {
        let columns = table_columns(items);
        if !columns.is_empty() {
            let table_rows = items
                .iter()
                .take(MAX_TABLE_ROWS)
                .map(|item| {
                    columns
                        .iter()
                        .map(|column| {
                            let value = item.get(column).unwrap_or(&Value::Null);
                            cell_for(column, value)
                        })
                        .collect()
                })
                .collect();
            let table = SimpleTable::new(
                format!("{id}-{depth}-{}", key.unwrap_or("rows")),
                columns
                    .iter()
                    .map(|column| Column::new(column.clone()))
                    .collect(),
                table_rows,
            )
            .empty("No rows", None)
            .render(cx);

            return match key {
                Some(key) => section(key, table, cx),
                None => table,
            };
        }
    }

    let theme = cx.theme();
    let chips = h_flex()
        .w_full()
        .flex_wrap()
        .gap_2()
        .children(items.iter().take(40).map(|item| {
            div()
                .px(px(8.))
                .py(px(2.))
                .rounded_full()
                .bg(theme.muted)
                .text_size(px(12.))
                .text_color(theme.foreground)
                .child(SharedString::from(scalar(item)))
        }))
        .into_any_element();

    match key {
        Some(key) => section(key, chips, cx),
        None => chips,
    }
}

fn section(title: &str, content: AnyElement, cx: &App) -> AnyElement {
    let theme = cx.theme();
    v_flex()
        .w_full()
        .gap_2()
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.muted_foreground)
                .child(SharedString::from(title.to_string())),
        )
        .child(content)
        .into_any_element()
}

fn kv_row(key: SharedString, value: &Value, cx: &App) -> Div {
    let theme = cx.theme();
    h_flex()
        .w_full()
        .items_start()
        .gap_3()
        .py(px(8.))
        .border_b_1()
        .border_color(theme.table_row_border)
        .child(
            div()
                .w(px(180.))
                .flex_shrink_0()
                .text_size(px(12.))
                .text_color(theme.muted_foreground)
                .child(key.clone()),
        )
        .child({
            let text = scalar(value);
            let key_lower = key.to_ascii_lowercase();
            let is_status = key_lower.ends_with("status")
                || key_lower.ends_with("state")
                || key_lower.ends_with("probe")
                || key_lower == "result";
            let mut cell = div()
                .flex_1()
                .min_w_0()
                .text_size(px(13.))
                .text_color(theme.foreground);
            if is_status && text.chars().count() <= 24 {
                cell = cell.child(pill(text.clone(), Tone::from_status(&text), cx));
            } else {
                cell = cell.child(SharedString::from(text));
            }
            cell
        })
}

fn cell_for(column: &str, value: &Value) -> Cell {
    let text = scalar(value);
    let lower = column.to_ascii_lowercase();
    if lower.ends_with("status") || lower.ends_with("state") || lower == "result" {
        Cell::status(text)
    } else if value.is_null() {
        Cell::muted("—")
    } else {
        Cell::new(text)
    }
}

fn table_columns(items: &[Value]) -> Vec<String> {
    let mut columns: Vec<String> = Vec::new();
    for item in items {
        let Some(map) = item.as_object() else {
            continue;
        };
        for (key, value) in map {
            if value.is_object() || value.is_array() {
                continue;
            }
            if !columns.iter().any(|existing| existing == key) {
                columns.push(key.clone());
            }
            if columns.len() >= MAX_TABLE_COLUMNS {
                return columns;
            }
        }
    }
    columns
}

fn scalar(value: &Value) -> String {
    let text = match value {
        Value::Null => "—".to_string(),
        Value::String(text) => text.clone(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        other => other.to_string(),
    };
    if text.chars().count() > MAX_VALUE_CHARS {
        let truncated: String = text.chars().take(MAX_VALUE_CHARS).collect();
        format!("{truncated}…")
    } else {
        text
    }
}

fn muted(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .py(px(4.))
        .text_size(px(12.))
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_formats_nulls_and_strings() {
        assert_eq!(scalar(&Value::Null), "—");
        assert_eq!(scalar(&Value::String("ok".into())), "ok");
        assert_eq!(scalar(&Value::Bool(true)), "true");
    }

    #[test]
    fn table_columns_skip_nested_values() {
        let items = vec![serde_json::json!({
            "id": 1,
            "status": "ok",
            "payload": {"nested": true},
        })];
        assert_eq!(table_columns(&items), vec!["id", "status"]);
    }

    #[test]
    fn table_columns_are_capped() {
        let mut map = serde_json::Map::new();
        for index in 0..20 {
            map.insert(format!("k{index}"), Value::from(index));
        }
        let items = vec![Value::Object(map)];
        assert_eq!(table_columns(&items).len(), MAX_TABLE_COLUMNS);
    }
}
