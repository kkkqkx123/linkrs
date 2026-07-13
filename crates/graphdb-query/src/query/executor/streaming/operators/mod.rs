//! Domain-specific operator enums

pub mod base;
pub mod spec;
pub mod state;

pub mod apply_operator;
pub mod blocking_operator;
pub mod ddl_operator;
pub mod exchange_operator;
pub mod fulltext_operator;
pub mod gather_operator;
pub mod graph_operator;
pub mod join_operator;
pub mod set_operator;
pub mod shuffle_join_operator;
pub mod sink_operator;
pub mod source_operator;
pub mod txn_operator;
pub mod unary_operator;
pub mod vector_operator;
