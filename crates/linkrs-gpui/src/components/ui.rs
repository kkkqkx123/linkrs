#![allow(unused_imports)]
#![allow(dead_code)]

//! Shared presentation primitives.
//!
//! These wrap gpui-kit components so every panel keeps one card style, one
//! table style and one status vocabulary. Panels should not hand-assemble
//! containers or semantic colours.

mod card;
mod data_view;
mod kpi;
mod loading;
mod overlay;
mod page;
mod pill;
mod table;

pub use card::*;
pub use data_view::*;
pub use kpi::*;
pub use loading::*;
pub use overlay::*;
pub use page::*;
pub use pill::*;
pub use table::*;
