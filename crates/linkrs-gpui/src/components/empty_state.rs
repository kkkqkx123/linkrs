use gpui_kit::assets::IconName;
use gpui_kit::component::{v_flex, ActiveTheme as _, Icon};
use gpui_kit::{
    div, AnyElement, App, FontWeight, IntoElement, ParentElement as _, SharedString, Styled as _,
};

/// Centered placeholder for panels with nothing to show yet.
pub fn empty_state(
    icon: IconName,
    title: impl Into<SharedString>,
    hint: Option<SharedString>,
    cx: &App,
) -> AnyElement {
    let muted = cx.theme().muted_foreground;

    div()
        .flex()
        .flex_1()
        .min_h_0()
        .items_center()
        .justify_center()
        .child(
            v_flex()
                .items_center()
                .gap_2()
                .child(Icon::new(icon).size_8().text_color(muted))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child(title.into()),
                )
                .children(hint.map(|hint| div().text_xs().text_color(muted).child(hint))),
        )
        .into_any_element()
}
