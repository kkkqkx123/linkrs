//! Vertex Table Persistence Layer
//!
//! Handles serialization, deserialization, and file I/O for vertex tables.
//!
//! # Encoding Handling
//! - Encodings are serialized as structured metadata during flush
//! - Encodings are reconstructed directly via `deserialize_meta()` on load

mod common;
mod delta;
mod encoding_select;
mod flush;
mod flush_incremental;
mod load;

pub use encoding_select::COLUMNS_FORMAT_VERSION;
