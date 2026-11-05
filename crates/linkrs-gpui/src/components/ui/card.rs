use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _};
use gpui_kit::{
    div, px, AnyElement, App, Div, FontWeight, ParentElement as _, SharedString, Styled as _,
};

use crate::design_tokens;
use crate::metrics::{CARD_HEAD_H, CARD_PAD, CARD_RADIUS};

/// Surface container: white card on the grey page canvas. The hairline uses
/// the light border token and the elevation matches the web studio's
/// `--shadow-sm`, including its stronger dark-appearance variant.
pub fn card(cx: &App) -> Div {
    let theme = cx.theme();
    let dark = theme.mode.is_dark();
    div()
        .bg(theme.popover)
        .border_1()
        .border_color(design_tokens::color("--color-border-light", dark))
        .rounded(px(CARD_RADIUS))
        .shadow(design_tokens::card_shadow(dark))
}

/// Card with the standard inner padding.
pub fn card_padded(cx: &App, dense: bool) -> Div {
    let pad = if dense {
        crate::metrics::CARD_PAD_DENSE
    } else {
        CARD_PAD
    };
    card(cx).p(px(pad))
}

/// Card header: 16px title, optional trailing actions, hairline divider.
pub fn card_head(title: impl Into<SharedString>, actions: Option<AnyElement>, cx: &App) -> Div {
    let theme = cx.theme();
    h_flex()
        .h(px(CARD_HEAD_H))
        .w_full()
        .flex_shrink_0()
        .items_center()
        .justify_between()
        .gap_3()
        .px(px(20.))
        .border_b_1()
        .border_color(design_tokens::color(
            "--color-border-light",
            theme.mode.is_dark(),
        ))
        .child(
            div()
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child(title.into()),
        )
        .children(actions)
}

/// Card body with padding following the row density.
pub fn card_body(dense: bool) -> Div {
    let pad = if dense {
        crate::metrics::CARD_PAD_DENSE
    } else {
        CARD_PAD
    };
    v_flex().w_full().p(px(pad)).min_w_0()
}

/// Stacked card list used as the page body.
pub fn card_stack() -> Div {
    v_flex().w_full().gap(px(crate::metrics::SECTION_GAP))
}
