use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _};
use gpui_kit::{
    div, px, AnyElement, App, Div, FontWeight, InteractiveElement as _, ParentElement as _,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _,
};

use crate::metrics::{CONTENT_MAX_W, PAGE_PAD_X, PAGE_PAD_Y};

/// Panel header: 16px title, optional subtitle and trailing actions.
pub fn panel_header(
    title: impl Into<SharedString>,
    subtitle: Option<SharedString>,
    actions: Option<AnyElement>,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let mut titles = v_flex().gap_0p5().min_w_0().child(
        div()
            .text_size(px(16.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.foreground)
            .child(title.into()),
    );
    if let Some(subtitle) = subtitle {
        titles = titles.child(
            div()
                .text_size(px(12.))
                .text_color(theme.muted_foreground)
                .child(subtitle),
        );
    }

    h_flex()
        .h(px(52.))
        .w_full()
        .flex_shrink_0()
        .items_center()
        .justify_between()
        .gap_3()
        .px(px(PAGE_PAD_X))
        .border_b_1()
        .border_color(theme.border)
        .child(titles)
        .children(actions)
}

/// Scrollable panel body with the page padding.
pub fn page_body(id: &'static str, _cx: &App) -> Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .min_h_0()
        .w_full()
        .overflow_y_scroll()
        .px(px(PAGE_PAD_X))
        .py(px(PAGE_PAD_Y))
}

/// Scrollable body that centers its content and caps the width.
pub fn document_body(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .min_h_0()
        .w_full()
        .overflow_y_scroll()
        .px(px(PAGE_PAD_X))
        .py(px(PAGE_PAD_Y))
}

/// Width-constrained content column for document-style pages.
pub fn document_column() -> Div {
    v_flex()
        .w_full()
        .max_w(px(CONTENT_MAX_W))
        .mx_auto()
        .gap(px(crate::metrics::SECTION_GAP))
}

/// Panel-local toolbar above a table or canvas.
pub fn toolbar(cx: &App) -> Div {
    h_flex()
        .w_full()
        .flex_shrink_0()
        .items_center()
        .gap_2()
        .px(px(16.))
        .py(px(10.))
        .border_b_1()
        .border_color(cx.theme().border)
}

/// Row of toolbar controls pushed to the trailing edge.
pub fn toolbar_actions() -> Div {
    h_flex().items_center().gap_2().ml_auto()
}

/// Empty scroll body used when a panel has no content to show.
pub fn centered_body() -> Div {
    div()
        .flex_1()
        .min_h_0()
        .w_full()
        .items_center()
        .justify_center()
}
