use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, StyledExt as _};
use gpui_kit::{
    div, px, App, Div, FontWeight, IntoElement, ParentElement as _, SharedString, Styled as _,
};

use super::card::card;
use super::pill::Tone;

/// One statistic tile.
pub struct Kpi {
    pub label: SharedString,
    pub value: SharedString,
    pub tone: Option<Tone>,
    pub hint: Option<SharedString>,
}

impl Kpi {
    pub fn new(label: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            tone: None,
            hint: None,
        }
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = Some(tone);
        self
    }

    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// Responsive grid of statistic tiles (four per row on wide panels).
pub fn kpi_grid(items: Vec<Kpi>, cx: &App) -> Div {
    let theme = cx.theme();
    h_flex()
        .w_full()
        .flex_wrap()
        .gap_3()
        .children(items.into_iter().map(|item| {
            let value_color = item
                .tone
                .map(|tone| super::pill::tone_color(theme, tone))
                .unwrap_or(theme.foreground);
            card(cx)
                .flex_1()
                .min_w(px(160.))
                .v_flex()
                .gap_0()
                .p(px(16.))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.muted_foreground)
                        .child(item.label),
                )
                .child(
                    div()
                        .mt(px(16.))
                        .text_size(px(24.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(value_color)
                        .child(item.value),
                )
                .children(item.hint.map(|hint| {
                    div()
                        .mt(px(4.))
                        .text_size(px(12.))
                        .text_color(theme.muted_foreground)
                        .child(hint)
                }))
        }))
}

/// Compact inline metric used in toolbars and card headers.
pub fn metric_inline(
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .gap_1()
        .items_center()
        .child(
            div()
                .text_size(px(12.))
                .text_color(theme.muted_foreground)
                .child(label.into()),
        )
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.foreground)
                .child(value.into()),
        )
}
