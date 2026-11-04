use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{v_flex, ActiveTheme as _, Sizable as _};
use gpui_kit::{div, px, App, Div, ParentElement as _, Styled as _};

/// Skeleton rows shown while a table's first page is loading.
pub fn skeleton_rows(count: usize, cx: &App) -> Div {
    let theme = cx.theme();
    let widths = [92.0, 78.0, 84.0, 65.0, 88.0, 72.0];
    v_flex()
        .w_full()
        .p(px(16.))
        .gap_3()
        .children((0..count).map(|row| {
            let width = widths[row % widths.len()];
            div()
                .h(px(14.))
                .w(px(220.) * (width / 100.0))
                .rounded(px(3.))
                .bg(theme.skeleton)
        }))
}

/// Centered spinner with an optional caption.
pub fn loading_block(label: impl Into<gpui_kit::SharedString>, cx: &App) -> Div {
    let theme = cx.theme();
    v_flex()
        .flex_1()
        .min_h(px(120.))
        .items_center()
        .justify_center()
        .gap_2()
        .child(Spinner::new().with_size(px(20.)).color(theme.primary))
        .child(
            div()
                .text_size(px(12.))
                .text_color(theme.muted_foreground)
                .child(label.into()),
        )
}
