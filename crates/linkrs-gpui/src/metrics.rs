//! Shared layout metrics for the desktop client.
//!
//! Values that have a counterpart in the web studio are checked against
//! `design/tokens.json` by the tests in `design_tokens`, so a token change on
//! either client fails the build instead of silently drifting. The remaining
//! constants describe desktop-only surfaces (the dock area, the status bar and
//! the expanded sidebar drag range) and are documented here.

/// Horizontal page padding inside a panel body.
pub const PAGE_PAD_X: f32 = 28.0;
/// Vertical page padding inside a panel body.
pub const PAGE_PAD_Y: f32 = 32.0;
/// Card inner padding at comfortable density.
pub const CARD_PAD: f32 = 24.0;
/// Card inner padding at dense density.
pub const CARD_PAD_DENSE: f32 = 16.0;
/// Card corner radius.
pub const CARD_RADIUS: f32 = 6.0;
/// Card header height.
pub const CARD_HEAD_H: f32 = 52.0;
/// Workspace header height, matching `--comp-height-top` in the web studio.
pub const HEADER_H: f32 = 58.0;
/// Default expanded sidebar width, matching the web studio's default rail.
pub const SIDEBAR_W: f32 = 176.0;
/// Minimum expanded sidebar width while resizing. The desktop client switches
/// to a dedicated icon rail instead of shrinking the expanded rail further.
pub const SIDEBAR_MIN: f32 = 180.0;
/// Maximum sidebar width while resizing.
pub const SIDEBAR_MAX: f32 = 300.0;
/// Comfortable table row height: 16px + 22px line height + 16px padding.
pub const ROW_H_COMFORTABLE: f32 = 54.0;
/// Dense table row height: 4px + 22px line height + 4px padding.
pub const ROW_H_DENSE: f32 = 30.0;
/// Maximum content width for document-style pages.
pub const CONTENT_MAX_W: f32 = 1500.0;
/// Table header row height, matching the comfortable body row.
pub const HEADER_ROW_H: f32 = 54.0;
/// Gap between stacked cards.
pub const SECTION_GAP: f32 = 16.0;
