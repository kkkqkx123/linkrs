//! Shared layout metrics for the desktop client.
//!
//! Values mirror the design tokens of the web studio (`frontend/src/app.css`)
//! so panels keep the same visual rhythm without scattering magic numbers.

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
/// Workspace header height above the dock area.
pub const HEADER_H: f32 = 52.0;
/// Default expanded sidebar width.
pub const SIDEBAR_W: f32 = 208.0;
/// Minimum sidebar width while resizing.
pub const SIDEBAR_MIN: f32 = 180.0;
/// Maximum sidebar width while resizing.
pub const SIDEBAR_MAX: f32 = 300.0;
/// Table row height at comfortable density.
pub const ROW_H_COMFORTABLE: f32 = 40.0;
/// Table row height at dense density.
pub const ROW_H_DENSE: f32 = 30.0;
/// Maximum content width for document-style pages.
pub const CONTENT_MAX_W: f32 = 1500.0;
/// Table header row height.
pub const HEADER_ROW_H: f32 = 36.0;
/// Gap between stacked cards.
pub const SECTION_GAP: f32 = 16.0;
