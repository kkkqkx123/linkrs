use gpui_kit::component::{h_flex, ActiveTheme as _, Theme};
use gpui_kit::{div, px, App, Div, Hsla, ParentElement as _, SharedString, Styled as _};

use crate::design_tokens;

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

    fn light_token(self) -> Option<&'static str> {
        match self {
            Tone::Neutral => None,
            Tone::Primary => Some("--color-primary-light"),
            Tone::Info => Some("--color-info-light"),
            Tone::Success => Some("--color-success-light"),
            Tone::Warning => Some("--color-warning-light"),
            Tone::Danger => Some("--color-error-light"),
            Tone::Alt => Some("--color-alt-light"),
        }
    }

    fn text_token(self) -> Option<&'static str> {
        match self {
            Tone::Warning => Some("--color-warning-text"),
            Tone::Danger => Some("--color-error-text"),
            _ => None,
        }
    }
}

/// Resolve a tone to its saturated colour, used for dots, table cells and KPI
/// values that sit on a plain surface.
pub fn tone_color(theme: &Theme, tone: Tone) -> Hsla {
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

/// Exact light background from the shared token catalogue. The web studio
/// defines dedicated dark-appearance values, so deriving these from the
/// saturated colour with a fixed alpha would be wrong in dark mode.
pub fn tone_background(theme: &Theme, tone: Tone) -> Hsla {
    match tone.light_token() {
        Some(token) => design_tokens::color(token, theme.mode.is_dark()),
        None => theme.muted,
    }
}

/// Foreground colour for text placed on a `tone_background` surface. Warning
/// and danger have dedicated darker text tokens for legibility.
pub fn tone_text(theme: &Theme, tone: Tone) -> Hsla {
    match tone.text_token() {
        Some(token) => design_tokens::color(token, theme.mode.is_dark()),
        None => tone_color(theme, tone),
    }
}

/// Text pill: semantic background with matching text and hairline border.
pub fn pill(text: impl Into<SharedString>, tone: Tone, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .flex_shrink_0()
        .px(px(12.))
        .py(px(2.))
        .rounded_full()
        .border_1()
        .border_color(tone_color(theme, tone).opacity(0.4))
        .bg(tone_background(theme, tone))
        .text_size(px(14.))
        .text_color(tone_text(theme, tone))
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
                .text_size(px(14.))
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
        .rounded(px(3.))
        .border_1()
        .border_color(if active { theme.primary } else { theme.border })
        .bg(if active { theme.primary } else { theme.popover })
        .text_size(px(12.))
        .text_color(if active {
            theme.primary_foreground
        } else {
            theme.muted_foreground
        })
        .child(text.into())
}
