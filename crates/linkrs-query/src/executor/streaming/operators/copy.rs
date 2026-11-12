//! COPY statement execution.
//!
//! `COPY FROM` imports CSV records into vertices or edges, `COPY TO`
//! exports them back out. The two directions share nothing but the
//! statement name, so each lives in its own submodule.

mod from;
mod source;
mod to;

pub use from::execute_copy_from;
pub use to::execute_copy_to;
