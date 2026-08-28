//! Engine comparison tests have been removed.
//!
//! Inversearch engine has been removed from the codebase. All fulltext search
//! now uses tantivy (BM25) exclusively. Comparison tests are no longer needed.
//! Basic CRUD, search limit, empty search, and special characters scenarios
//! are covered by basic.rs and edge_cases.rs.
