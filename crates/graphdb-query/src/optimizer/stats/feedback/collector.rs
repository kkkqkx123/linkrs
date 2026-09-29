//! Legacy feedback collectors removed.
//!
//! Production execution feedback flows through `history::QueryFeedbackHistory`
//! and `query::QueryExecutionFeedback`, folded by `engine::feedback`.
//! This module remains as a placeholder so `feedback::collector` paths keep
//! resolving while the old counter types stay deleted.
