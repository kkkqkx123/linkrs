//! Space management actuators
//!
//! Provides functions for creating, deleting, modifying, emptying, describing, listing and switching diagram spaces.

pub mod alter_space;
pub mod clear_space;
pub mod create_space;
pub mod desc_space;
pub mod drop_space;
pub mod show_spaces;
pub mod switch_space;

#[cfg(test)]
mod tests;

pub use alter_space::{AlterSpaceExecutor, SpaceAlterOption};
pub use clear_space::ClearSpaceExecutor;
pub use create_space::CreateSpaceExecutor;
pub use desc_space::DescSpaceExecutor;
pub use drop_space::DropSpaceExecutor;
pub use show_spaces::ShowSpacesExecutor;
pub use switch_space::SwitchSpaceExecutor;
