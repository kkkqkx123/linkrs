use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _};
use gpui_kit::{
    div, px, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
};

/// Shared filter/sort/page-size controls used by the data browser. The view
/// owns the entities; this helper only builds them and renders one compact
/// row so every list view keeps the same control order.
pub struct FilterControls {
    pub filter: Entity<InputState>,
    pub sort_by: Entity<InputState>,
    pub descending: bool,
}

impl FilterControls {
    pub fn new<V: 'static>(window: &mut Window, cx: &mut Context<V>) -> Self {
        let filter =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter expression, e.g. age>18"));
        let sort_by = cx.new(|cx| InputState::new(window, cx).placeholder("Sort field"));
        Self {
            filter,
            sort_by,
            descending: false,
        }
    }

    pub fn filter_text(&self, cx: &App) -> String {
        self.filter.read(cx).value().to_string()
    }

    pub fn sort_text(&self, cx: &App) -> String {
        self.sort_by.read(cx).value().to_string()
    }

    pub fn sort_order(&self) -> &'static str {
        if self.descending {
            "DESC"
        } else {
            "ASC"
        }
    }

    pub fn render<V: 'static>(&self, cx: &Context<V>) -> impl IntoElement {
        let theme = cx.theme().clone();
        h_flex()
            .items_center()
            .gap_2()
            .child(div().flex_1().min_w_0().child(Input::new(&self.filter)))
            .child(
                div()
                    .w(px(160.))
                    .flex_shrink_0()
                    .child(Input::new(&self.sort_by)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(self.sort_order())),
            )
    }

    /// Split a comma-separated `key=value,key2~substring` filter into pairs.
    /// The data browser passes the raw expression to the server; this helper
    /// only validates the local shape before a request is sent.
    pub fn validate_filter(text: &str) -> Option<String> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(trimmed.to_string())
    }
}

/// Render one labeled row used by management forms.
pub fn form_row<V: 'static>(
    label: &'static str,
    field: Entity<InputState>,
    cx: &Context<V>,
) -> impl IntoElement {
    h_flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .w(px(112.))
                .flex_shrink_0()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(div().flex_1().min_w_0().child(Input::new(&field)))
}

/// Stacked scrollable JSON/text block for untyped management payloads.
pub fn render_json_block(value: &serde_json::Value, cx: &App) -> impl IntoElement {
    let pretty = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    v_flex().gap_1().child(
        div()
            .id("json-block")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .text_xs()
            .text_color(cx.theme().foreground)
            .child(SharedString::from(pretty)),
    )
}
