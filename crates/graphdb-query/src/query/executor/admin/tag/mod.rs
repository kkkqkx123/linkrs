//! Tag Management Executor
//!
//! Provide functions for creating, modifying, describing, deleting, and listing tags.

pub mod alter_tag;
pub mod create_tag;
pub mod desc_tag;
pub mod drop_tag;
pub mod show_create_tag;
pub mod show_tags;

#[cfg(test)]
mod tests;

pub use alter_tag::AlterTagExecutor;
pub use create_tag::CreateTagExecutor;
pub use desc_tag::DescTagExecutor;
pub use drop_tag::DropTagExecutor;
pub use show_create_tag::ShowCreateTagExecutor;
pub use show_tags::ShowTagsExecutor;
