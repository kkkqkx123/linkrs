use gpui_kit::component::{h_flex, ActiveTheme as _, Theme};
use gpui_kit::{div, px, App, Div, FontWeight, ParentElement as _, SharedString, Styled as _};

/// Semantic tone shared by pills, status dots and table cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Primary,
    Info,
    Success,
    Warning,
    Danger,
    Alt,
}

impl Tone {
    /// Pick the tone matching a server-reported status word.
    pub fn from_status(status: &str) -> Self {
        let status = status.trim().to_ascii_lowercase();
        match status.as_str() {
            "ok" | "success" | "succeeded" | "done" | "active" | "connected" | "reachable"
            | "applied" | "enabled" | "healthy" => Self::Success,
            "warn" | "warning" | "degraded" | "pending" | "running" | "in_progress"
            | "unhealthy" => Self::Warning,
            "error" | "failed" | "failure" | "fatal" | "unreachable" | "disabled"
            | "rolled_back" => Self::Danger,
            "info" | "info_only" => Self::Info,
            _ => Self::Neutral,
        }
    }
}

/// Resolve a tone to its foreground colour.
pub fn tone_color(theme: &Theme, tone: Tone) -> gpui_kit::Hsla {
    match tone {
        Tone::Neutral => theme.muted_foreground,
        Tone::Primary => theme.primary,
        Tone::Info => theme.info,
        Tone::Success => theme.success,
        Tone::Warning => theme.warning,
        Tone::Danger => theme.danger,
        Tone::Alt => theme.chart_4,
    }
}

fn tone_background(theme: &Theme, tone: Tone) -> gpui_kit::Hsla {
    match tone {
        Tone::Neutral => theme.muted,
        other => tone_color(theme, other).opacity(0.12),
    }
}

/// Text pill: semantic background wash with the tone as text colour.
pub fn pill(text: impl Into<SharedString>, tone: Tone, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .flex_shrink_0()
        .px(px(8.))
        .py(px(2.))
        .rounded_full()
        .bg(tone_background(theme, tone))
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(tone_color(theme, tone))
        .child(text.into())
}

/// Status dot followed by a label.
pub fn status_dot(
    text: impl Into<SharedString>,
    tone: Tone,
    cx: &App,
) -> impl gpui_kit::IntoElement {
    let theme = cx.theme();
    let color = tone_color(theme, tone);
    h_flex()
        .items_center()
        .gap_1p5()
        .child(div().size(px(7.)).rounded_full().bg(color))
        .child(
            div()
                .text_size(px(12.))
                .text_color(theme.foreground)
                .child(text.into()),
        )
}

/// Toggleable filter chip.
pub fn chip(text: impl Into<SharedString>, active: bool, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .flex_shrink_0()
        .px(px(10.))
        .h(px(24.))
        .flex()
        .items_center()
        .rounded_full()
        .border_1()
        .border_color(if active { theme.primary } else { theme.border })
        .bg(if active { theme.accent } else { theme.popover })
        .text_size(px(12.))
        .text_color(if active {
            theme.primary
        } else {
            theme.muted_foreground
        })
        .child(text.into())
}
