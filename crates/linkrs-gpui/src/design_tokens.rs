//! Machine-readable design tokens shared with the web studio.
//!
//! `design/tokens.json` is the single source of truth. The web stylesheet is
//! checked against it by `frontend/scripts/check-design-tokens.mjs`, and the
//! desktop client reads the same file at runtime so the two clients cannot
//! drift apart through hand-copied colour constants.

use std::collections::HashMap;
use std::sync::LazyLock;

use gpui_kit::component::box_shadow;
use gpui_kit::{px, rgb, rgba, BoxShadow, Hsla};
use serde::Deserialize;

const TOKEN_JSON: &str = include_str!("../../../design/tokens.json");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesktopTokens {
    primary_active_light: String,
    primary_active_dark: String,
    font_family: String,
}

#[derive(Deserialize)]
struct TokenFile {
    theme: HashMap<String, String>,
    root: HashMap<String, String>,
    dark: HashMap<String, String>,
    desktop: DesktopTokens,
}

static TOKENS: LazyLock<TokenFile> =
    LazyLock::new(|| serde_json::from_str(TOKEN_JSON).expect("bundled design tokens parse"));

fn token_file() -> &'static TokenFile {
    LazyLock::force(&TOKENS)
}

fn lookup<'a>(
    section: &'a HashMap<String, String>,
    fallback: Option<&'a HashMap<String, String>>,
    name: &str,
) -> &'a str {
    section
        .get(name)
        .or_else(|| fallback.and_then(|map| map.get(name)))
        .map(String::as_str)
        .expect("design token is declared in the shared catalogue")
}

/// Look up a CSS custom property, falling back to the light theme for tokens
/// that the dark appearance intentionally inherits (the navigation rail).
pub fn value(name: &str, dark: bool) -> &'static str {
    let file = token_file();
    if dark {
        lookup(&file.dark, Some(&file.theme), name)
    } else {
        lookup(&file.theme, None, name)
    }
}

/// Parse a CSS colour into GPUI's colour type.
fn parse_color(raw: &str) -> Hsla {
    let text = raw.trim();
    if let Some(hex) = text.strip_prefix('#') {
        let expanded = match hex.len() {
            3 => hex.chars().flat_map(|c| [c, c]).collect::<String>(),
            6 | 8 => hex.to_string(),
            _ => panic!("unsupported hex colour in design tokens: {text}"),
        };
        let packed =
            u32::from_str_radix(&expanded, 16).expect("hex colour in design tokens is valid");
        return if expanded.len() == 8 {
            rgba(packed).into()
        } else {
            rgb(packed).into()
        };
    }
    if let Some(body) = text
        .strip_prefix("rgba(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let channels: Vec<f32> = body
            .split(',')
            .map(|part| match part.trim().parse::<f32>() {
                Ok(number) => number,
                Err(_) => 0.,
            })
            .collect();
        if channels.len() == 4 {
            let channel = |value: f32| value.clamp(0., 255.).round() as u32;
            let alpha = (channels[3].clamp(0., 1.) * 255.).round() as u32;
            let packed = (channel(channels[0]) << 24)
                | (channel(channels[1]) << 16)
                | (channel(channels[2]) << 8)
                | alpha;
            return rgba(packed).into();
        }
    }
    panic!("unsupported colour in design tokens: {text}");
}

/// Resolve a semantic colour token for the active appearance.
pub fn color(name: &str, dark: bool) -> Hsla {
    parse_color(value(name, dark))
}

/// Primary pressed colour, which has no web counterpart because CSS handles
/// it through `:active` utilities.
pub fn primary_active(dark: bool) -> Hsla {
    let file = token_file();
    let name = if dark {
        &file.desktop.primary_active_dark
    } else {
        &file.desktop.primary_active_light
    };
    parse_color(name)
}

/// CJK-aware font stack used by the web studio.
pub fn font_family() -> &'static str {
    &token_file().desktop.font_family
}

/// Parse a pixel-valued metric from the shared `:root` block.
pub fn metric(name: &str) -> f32 {
    let raw = lookup(&token_file().root, None, name);
    let digits = raw.trim().trim_end_matches("px").trim();
    digits
        .parse::<f32>()
        .expect("pixel metric in design tokens is numeric")
}

/// The web studio's `--shadow-sm`, expressed as GPUI box shadows. Both the
/// offsets and the alpha values come from the token catalogue, including the
/// stronger dark-appearance variant.
pub fn card_shadow(dark: bool) -> Vec<BoxShadow> {
    let file = token_file();
    let raw = if dark {
        lookup(&file.dark, Some(&file.root), "--shadow-sm")
    } else {
        lookup(&file.root, None, "--shadow-sm")
    };

    let lengths: Vec<f32> = raw
        .split_whitespace()
        .filter_map(|part| part.strip_suffix("px"))
        .filter_map(|number| number.parse::<f32>().ok())
        .collect();

    let mut colors = Vec::new();
    let mut rest = raw;
    while let Some(start) = rest.find("rgba(") {
        let tail = &rest[start..];
        match tail.find(')') {
            Some(end) => {
                colors.push(parse_color(&tail[..=end]));
                rest = &tail[end + 1..];
            }
            None => break,
        }
    }

    if lengths.len() < 4 || colors.len() < 2 {
        panic!("--shadow-sm in design tokens must define two shadow layers");
    }

    vec![
        box_shadow(px(0.), px(lengths[0]), px(lengths[1]), px(0.), colors[0]),
        box_shadow(px(0.), px(lengths[2]), px(lengths[3]), px(0.), colors[1]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{CARD_PAD, CARD_PAD_DENSE, CARD_RADIUS, HEADER_H, PAGE_PAD_X, PAGE_PAD_Y};

    #[test]
    fn light_palette_matches_web_tokens() {
        assert_eq!(color("--color-primary", false), rgb(0x299c68).into());
        assert_eq!(color("--color-bg", false), rgb(0xf4f6f8).into());
        assert_eq!(color("--color-bg-white", false), rgb(0xffffff).into());
        assert_eq!(color("--color-error", false), rgb(0xd54941).into());
        assert_eq!(color("--color-info", false), rgb(0x0052d9).into());
        assert_eq!(color("--color-border-light", false), rgb(0xe8e8e8).into());
    }

    #[test]
    fn dark_palette_matches_web_tokens() {
        assert_eq!(color("--color-primary", true), rgb(0x34b37a).into());
        assert_eq!(color("--color-bg", true), rgb(0x0b0f17).into());
        assert_eq!(color("--color-bg-white", true), rgb(0x1c2333).into());
        assert_eq!(color("--color-error-light", true), rgb(0x3d1d1b).into());
        assert_eq!(color("--color-warning-text", true), rgb(0xf0b26a).into());
    }

    #[test]
    fn navigation_rail_is_shared_across_appearances() {
        let light = color("--color-nav", false);
        let dark = color("--color-nav", true);
        assert_eq!(light, dark);
        assert_eq!(light, rgb(0x26343d).into());
        assert_eq!(color("--color-nav-accent", true), rgb(0x5bc28e).into());
    }

    #[test]
    fn alpha_text_and_mask_tokens_keep_their_opacity() {
        let text = color("--color-text", false);
        let mask = color("--color-mask", false);
        assert!(text.a < 1.0);
        assert!(mask.a > 0.0 && mask.a < 1.0);
    }

    #[test]
    fn metrics_match_shared_tokens() {
        assert_eq!(HEADER_H, metric("--comp-height-top"));
        assert_eq!(PAGE_PAD_X, metric("--size-9"));
        assert_eq!(PAGE_PAD_Y, metric("--size-10"));
        assert_eq!(CARD_PAD, metric("--size-8"));
        assert_eq!(CARD_PAD_DENSE, metric("--size-6"));
        assert_eq!(CARD_RADIUS, metric("--radius-card"));
    }

    #[test]
    fn card_shadow_uses_two_layers_from_tokens() {
        assert_eq!(card_shadow(false).len(), 2);
        assert_eq!(card_shadow(true).len(), 2);
        assert!(card_shadow(true)[0].color.a > card_shadow(false)[0].color.a);
    }
}
