//! Brand palette applied on top of the bundled GPUI Kit themes.
//!
//! The web studio is the visual reference: a green brand, a grey page canvas
//! with white cards, and a dark navigation rail shared by both appearances.
//! `apply` rewrites the semantic colour tokens once per theme switch so every
//! gpui-kit component inherits the same palette.

use gpui_kit::component::Theme;
use gpui_kit::{px, rgb, rgba, App, Hsla};

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

fn color(hex: u32) -> Hsla {
    rgb(hex).into()
}

impl Palette {
    fn light() -> Self {
        Self {
            page: color(0xf4f6f8),
            surface: color(0xffffff),
            muted: color(0xf3f3f3),
            text: color(0x1a1a1a),
            text_muted: color(0x666666),
            border: color(0xdddddd),
            border_light: color(0xe8e8e8),
            primary: color(0x299c68),
            primary_hover: color(0x238757),
            primary_active: color(0x1d7d52),
            primary_light: color(0xe8f5ee),
            info: color(0x0052d9),
            success: color(0x2ba471),
            warning: color(0xe37318),
            danger: color(0xd54941),
            alt: color(0x7c3aed),
            mask: rgba(0x00000033).into(),
        }
    }

    fn dark() -> Self {
        Self {
            page: color(0x0b0f17),
            surface: color(0x1c2333),
            muted: color(0x232b3f),
            text: color(0xe5e7eb),
            text_muted: color(0x9ca3af),
            border: color(0x454f63),
            border_light: color(0x2d3748),
            primary: color(0x34b37a),
            primary_hover: color(0x4cc08d),
            primary_active: color(0x3fa873),
            primary_light: color(0x1d3a2c),
            info: color(0x6b93f0),
            success: color(0x3cba85),
            warning: color(0xef8c3f),
            danger: color(0xe06059),
            alt: color(0xa78bfa),
            mask: rgba(0x00000080).into(),
        }
    }
}

/// The dark navigation rail is shared by both appearances.
fn nav() -> Hsla {
    color(0x26343d)
}

/// Apply the brand palette to the active theme.
///
/// Call after every `Theme::change`: switching modes reloads the registered
/// theme and drops these overrides.
pub fn apply(cx: &mut App) {
    let dark = Theme::global(cx).mode.is_dark();
    let palette = if dark {
        Palette::dark()
    } else {
        Palette::light()
    };
    let nav = nav();
    let white = color(0xffffff);

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

        theme.colors.tab_bar = palette.muted;
        theme.colors.tab_bar_segmented = palette.muted;
        theme.colors.tab_active = palette.surface;
        theme.colors.tab_active_foreground = palette.text;

        theme.colors.sidebar = nav;
        theme.colors.sidebar_foreground = color(0xd9e2e6);
        theme.colors.sidebar_accent = color(0x3a4b57);
        theme.colors.sidebar_accent_foreground = white;
        theme.colors.sidebar_border = color(0x3a4a54);
        theme.colors.sidebar_primary = color(0x31434e);
        theme.colors.sidebar_primary_foreground = color(0x5bc28e);

        theme.colors.selection = palette.primary_light;
        theme.colors.skeleton = palette.muted;
        theme.colors.overlay = palette.mask;
        theme.colors.progress_bar = palette.primary;

        theme.colors.chart_1 = palette.primary;
        theme.colors.chart_2 = palette.info;
        theme.colors.chart_3 = palette.warning;
        theme.colors.chart_4 = palette.alt;
        theme.colors.chart_5 = palette.danger;
        theme.colors.chart_grid = palette.border_light;
        theme.colors.chart_bullish = palette.success;
        theme.colors.chart_bearish = palette.danger;

        theme.radius = px(6.);
        theme.radius_lg = px(12.);
        theme.mono_font_size = px(13.);
        theme.shadow = true;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_palette_matches_frontend_tokens() {
        let palette = Palette::light();
        assert_eq!(palette.primary, color(0x299c68));
        assert_eq!(palette.page, color(0xf4f6f8));
        assert_eq!(palette.surface, color(0xffffff));
        assert_eq!(palette.danger, color(0xd54941));
        assert_eq!(palette.info, color(0x0052d9));
    }

    #[test]
    fn dark_palette_keeps_brand_green() {
        let palette = Palette::dark();
        assert_eq!(palette.primary, color(0x34b37a));
        assert_eq!(palette.page, color(0x0b0f17));
        assert_eq!(palette.surface, color(0x1c2333));
    }

    #[test]
    fn navigation_rail_is_shared() {
        assert_eq!(nav(), color(0x26343d));
    }
}
