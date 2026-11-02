use gpui_kit::assets::IconName;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon};
use gpui_kit::{
    div, prelude::FluentBuilder as _, px, App, IntoElement, ParentElement as _, SharedString,
    Styled as _,
};

/// Flat property row rendered by [`render_properties`].
pub struct PropertyRow {
    pub key: String,
    pub value: String,
}

/// Flatten one JSON value into display rows: objects recurse with dotted
/// keys, arrays render with indexed keys, scalars render verbatim. Depth
/// and breadth are capped so hostile payloads cannot blow up the panel.
pub fn flatten_properties(value: &serde_json::Value) -> Vec<PropertyRow> {
    const MAX_DEPTH: usize = 3;
    const MAX_ROWS: usize = 200;
    let mut rows = Vec::new();
    flatten_into(String::new(), value, 0, MAX_DEPTH, &mut rows);
    rows.truncate(MAX_ROWS);
    rows
}

fn flatten_into(
    prefix: String,
    value: &serde_json::Value,
    depth: usize,
    max_depth: usize,
    rows: &mut Vec<PropertyRow>,
) {
    match value {
        serde_json::Value::Object(map) => {
            if depth >= max_depth || map.is_empty() {
                rows.push(PropertyRow {
                    key: prefix,
                    value: value.to_string(),
                });
                return;
            }
            for (key, child) in map {
                let next = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten_into(next, child, depth + 1, max_depth, rows);
            }
        }
        serde_json::Value::Array(items) => {
            if depth >= max_depth || items.is_empty() {
                rows.push(PropertyRow {
                    key: prefix,
                    value: value.to_string(),
                });
                return;
            }
            for (ix, child) in items.iter().enumerate().take(20) {
                flatten_into(format!("{prefix}[{ix}]"), child, depth + 1, max_depth, rows);
            }
        }
        serde_json::Value::Null => rows.push(PropertyRow {
            key: prefix,
            value: "NULL".to_string(),
        }),
        serde_json::Value::String(text) => rows.push(PropertyRow {
            key: prefix,
            value: text.clone(),
        }),
        other => rows.push(PropertyRow {
            key: prefix,
            value: other.to_string(),
        }),
    }
}

/// Render a titled key/value property list with an empty state.
pub fn render_properties(
    title: &str,
    subtitle: Option<&str>,
    value: &serde_json::Value,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    let rows = flatten_properties(value);

    v_flex()
        .gap_1()
        .p_3()
        .child(
            div()
                .text_sm()
                .text_color(theme.foreground)
                .child(SharedString::from(title.to_string())),
        )
        .when_some(subtitle, |panel, subtitle| {
            panel.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(subtitle.to_string())),
            )
        })
        .when(rows.is_empty(), |panel| {
            panel.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .py_2()
                    .child(Icon::new(IconName::Inbox).text_color(theme.muted_foreground))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from("No properties")),
                    ),
            )
        })
        .children(rows.into_iter().map(|row| {
            h_flex()
                .gap_2()
                .items_start()
                .py_0p5()
                .child(
                    div()
                        .w(px(140.))
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(SharedString::from(row.key)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .text_color(theme.foreground)
                        .child(SharedString::from(row.value)),
                )
        }))
}

/// Render the selected graph vertex as a property panel.
pub fn render_graph_selection(
    title: Option<&str>,
    value: &serde_json::Value,
    cx: &App,
) -> impl IntoElement {
    match title {
        Some(title) => render_properties("Selection", Some(title), value, cx).into_any_element(),
        None => v_flex()
            .p_3()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(
                        "Select a node to inspect its properties.",
                    )),
            )
            .into_any_element(),
    }
}
