//! Brand palette applied on top of the bundled GPUI Kit themes.
//!
//! The web studio is the visual reference: a green brand, a grey page canvas
//! with white cards, and a dark navigation rail shared by both appearances.
//! Colours come from `design/tokens.json`, the same catalogue the web
//! stylesheet is checked against; `apply` rewrites the semantic colour tokens
//! once per theme switch so every gpui-kit component inherits the same palette.

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{px, App, Hsla, SharedString, Window};

use crate::design_tokens;
use crate::settings::ThemeModeSetting;

/// Semantic colours in frontend token order.
struct Palette {
    page: Hsla,
    surface: Hsla,
    muted: Hsla,
    text: Hsla,
    text_muted: Hsla,
    border: Hsla,
    border_light: Hsla,
    primary: Hsla,
    primary_hover: Hsla,
    primary_active: Hsla,
    primary_light: Hsla,
    info: Hsla,
    success: Hsla,
    warning: Hsla,
    danger: Hsla,
    alt: Hsla,
    mask: Hsla,
}

impl Palette {
    fn light() -> Self {
        Self::for_appearance(false)
    }

    fn dark() -> Self {
        Self::for_appearance(true)
    }

    fn for_appearance(dark: bool) -> Self {
        let color = |name: &str| design_tokens::color(name, dark);
        Self {
            page: color("--color-bg"),
            surface: color("--color-bg-white"),
            muted: color("--color-bg-secondary"),
            text: color("--color-text"),
            text_muted: color("--color-text-secondary"),
            border: color("--color-border"),
            border_light: color("--color-border-light"),
            primary: color("--color-primary"),
            primary_hover: color("--color-primary-hover"),
            primary_active: design_tokens::primary_active(dark),
            primary_light: color("--color-primary-light"),
            info: color("--color-info"),
            success: color("--color-success"),
            warning: color("--color-warning"),
            danger: color("--color-error"),
            alt: color("--color-alt"),
            mask: color("--color-mask"),
        }
    }
}

/// The dark navigation rail is shared by both appearances.
fn nav() -> Hsla {
    design_tokens::color("--color-nav", false)
}

/// Change the theme and reapply the brand overrides.
///
/// `Theme::change` reloads the registered theme and drops every override, so
/// callers must go through this single entry point instead of pairing the two
/// calls themselves.
pub fn set_mode(mode: ThemeModeSetting, window: Option<&mut Window>, cx: &mut App) {
    Theme::change(to_theme_mode(mode, cx), window, cx);
    apply(cx);
}

fn to_theme_mode(mode: ThemeModeSetting, cx: &App) -> ThemeMode {
    crate::app::to_theme_mode(mode, cx)
}

/// Apply the brand palette to the active theme.
pub fn apply(cx: &mut App) {
    let dark = Theme::global(cx).mode.is_dark();
    let palette = if dark {
        Palette::dark()
    } else {
        Palette::light()
    };
    let nav = nav();
    let white = design_tokens::color("--color-nav-brand", dark);
    let nav_text = design_tokens::color("--color-nav-text", dark);
    let nav_hover = design_tokens::color("--color-nav-hover", dark);
    let nav_border = design_tokens::color("--color-nav-border", dark);
    let nav_card = design_tokens::color("--color-nav-card", dark);
    let nav_accent = design_tokens::color("--color-nav-accent", dark);

    Theme::update(cx, |theme| {
        theme.colors.background = palette.page;
        theme.colors.foreground = palette.text;

        theme.colors.popover = palette.surface;
        theme.colors.popover_foreground = palette.text;
        theme.colors.secondary = palette.muted;
        theme.colors.secondary_hover = palette.muted;
        theme.colors.secondary_active = palette.muted;
        theme.colors.secondary_foreground = palette.text;
        theme.colors.muted = palette.muted;
        theme.colors.muted_foreground = palette.text_muted;
        theme.colors.group_box = palette.surface;
        theme.colors.group_box_foreground = palette.text;
        theme.colors.accordion = palette.surface;
        theme.colors.caret = palette.text_muted;

        theme.colors.border = palette.border;
        theme.colors.input = palette.border;
        theme.colors.scrollbar_thumb = palette.border;
        theme.colors.window_border = palette.border;

        theme.colors.primary = palette.primary;
        theme.colors.primary_hover = palette.primary_hover;
        theme.colors.primary_active = palette.primary_active;
        theme.colors.primary_foreground = white;
        theme.colors.accent = palette.primary_light;
        theme.colors.accent_foreground = palette.primary;
        theme.colors.ring = palette.primary;

        theme.colors.info = palette.info;
        theme.colors.info_hover = palette.info;
        theme.colors.info_active = palette.info;
        theme.colors.info_foreground = white;
        theme.colors.success = palette.success;
        theme.colors.success_hover = palette.success;
        theme.colors.success_active = palette.success;
        theme.colors.success_foreground = white;
        theme.colors.warning = palette.warning;
        theme.colors.warning_foreground = white;
        theme.colors.danger = palette.danger;
        theme.colors.danger_hover = palette.danger;
        theme.colors.danger_active = palette.danger;
        theme.colors.danger_foreground = white;

        theme.colors.link = palette.primary;
        theme.colors.link_hover = palette.primary_hover;
        theme.colors.link_active = palette.primary_active;

        theme.colors.table = palette.surface;
        theme.colors.table_even = palette.surface;
        theme.colors.table_head = palette.muted;
        theme.colors.table_head_foreground = palette.text_muted;
        theme.colors.table_hover = palette.muted;
        theme.colors.table_row_border = palette.border_light;
        theme.colors.table_active = palette.primary_light;
        theme.colors.table_active_border = palette.primary;

        theme.colors.list = palette.surface;
        theme.colors.list_even = palette.surface;
        theme.colors.list_head = palette.muted;
        theme.colors.list_hover = palette.muted;
        theme.colors.list_active = palette.primary_light;
        theme.colors.list_active_border = palette.primary;

        theme.colors.title_bar = palette.surface;
        theme.colors.title_bar_border = palette.border_light;
        theme.colors.status_bar = palette.surface;
        theme.colors.status_bar_border = palette.border_light;

        theme.colors.tab = palette.muted;
        theme.colors.tab_foreground = palette.text_muted;
        theme.colors.tab_bar = palette.muted;
        theme.colors.tab_bar_segmented = palette.muted;
        theme.colors.tab_active = palette.surface;
        theme.colors.tab_active_foreground = palette.primary;

        theme.colors.sidebar = nav;
        theme.colors.sidebar_foreground = nav_text;
        theme.colors.sidebar_accent = nav_hover;
        theme.colors.sidebar_accent_foreground = white;
        theme.colors.sidebar_border = nav_border;
        theme.colors.sidebar_primary = nav_card;
        theme.colors.sidebar_primary_foreground = nav_accent;

        theme.colors.selection = palette.primary_light;
        theme.colors.skeleton = palette.muted;
        theme.colors.overlay = palette.mask;
        theme.colors.progress_bar = palette.primary;

        theme.colors.chart_1 = palette.info;
        theme.colors.chart_2 = palette.primary;
        theme.colors.chart_3 = palette.danger;
        theme.colors.chart_4 = palette.alt;
        theme.colors.chart_5 = palette.warning;
        theme.colors.chart_grid = palette.border_light;
        theme.colors.chart_bullish = palette.success;
        theme.colors.chart_bearish = palette.danger;

        theme.font_size = px(design_tokens::metric("--font-size-base"));
        theme.font_family = SharedString::from(design_tokens::font_family());
        // Controls use the compact radius; cards keep their own 6px radius via
        // `metrics::CARD_RADIUS`, matching the web studio's 3px/6px split.
        theme.radius = px(design_tokens::metric("--radius-sm"));
        theme.radius_lg = px(design_tokens::metric("--radius-xl"));
        theme.mono_font_size = px(13.);
        theme.shadow = true;
        theme.focus_ring = true;
        theme.notification.width = px(384.);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::rgb;

    #[test]
    fn light_palette_matches_frontend_tokens() {
        let palette = Palette::light();
        assert_eq!(palette.primary, rgb(0x299c68).into());
        assert_eq!(palette.page, rgb(0xf4f6f8).into());
        assert_eq!(palette.surface, rgb(0xffffff).into());
        assert_eq!(palette.danger, rgb(0xd54941).into());
        assert_eq!(palette.info, rgb(0x0052d9).into());
        assert_eq!(palette.border_light, rgb(0xe8e8e8).into());
        assert!((palette.mask.a - 0.2).abs() < 0.01);
    }

    #[test]
    fn dark_palette_keeps_brand_green() {
        let palette = Palette::dark();
        assert_eq!(palette.primary, rgb(0x34b37a).into());
        assert_eq!(palette.page, rgb(0x0b0f17).into());
        assert_eq!(palette.surface, rgb(0x1c2333).into());
        assert_eq!(palette.text, rgb(0xe5e7eb).into());
        assert_eq!(palette.text_muted, rgb(0x9ca3af).into());
    }

    #[test]
    fn navigation_rail_is_shared() {
        assert_eq!(nav(), rgb(0x26343d).into());
        assert_eq!(nav(), design_tokens::color("--color-nav", true));
    }

    #[test]
    fn semantic_palette_covers_both_appearances() {
        for dark in [false, true] {
            let palette = Palette::for_appearance(dark);
            assert_ne!(palette.primary, palette.danger);
            assert_ne!(palette.border, palette.border_light);
            assert_ne!(palette.primary, palette.primary_hover);
            assert!(design_tokens::color("--color-error-light", dark) != palette.surface);
        }
    }
}
