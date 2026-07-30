// Buffer management module — Phase 2 of the storage optimization plan.
// Not yet integrated into the main read path; items are exported for
// use by the integration code once BufferManager wiring is complete.
#![allow(dead_code, unused_imports)]

pub mod eviction;
pub mod manager;
pub mod page;

pub use eviction::{EvictionQueue, EvictionStats};
pub use manager::{
    BufferCategory, BufferConfig, BufferManager, BufferStats, PageId,
};
pub use page::{BufferPage, Pageable, PageState};
