//! OperatorSpec: Immutable configuration descriptors for operator nodes.
//!
//! Each variant holds only the immutable fields of a corresponding operator
//! — expressions, configuration values, column names — but never cursors,
//! hash tables, buffers, or lifecycle state.  This makes an `OperatorSpec`
//! suitable for caching, EXPLAIN, and repeated instantiation without shared
//! mutable state.
//!
//! This module is a thin compatibility shim: every spec lives in its own
//! `spec/*` submodule mirroring one `OperatorKindSpec` domain, and this file
//! only declares those submodules and re-exports their public items so that
//! existing `operators::spec::SourceSpec` paths keep working.

pub mod apply;
pub mod blocking;
pub mod cardinality;
pub mod ddl;
pub mod exchange;
pub mod fulltext;
pub mod graph;
pub mod join;
pub mod recursive;
pub mod set;
pub mod sink;
pub mod source;
pub mod txn;
pub mod unary;
pub mod vector;

pub use apply::{ApplyKind, ApplySpec};
pub use blocking::BlockingSpec;
pub use cardinality::operator_cardinality_shape_key;
pub use ddl::{
    DdlSpec, EdgeManageCommand, IndexManageCommand, MigrateAction, PropertyRename,
    SequenceManageCommand, SpaceManageCommand, TagManageCommand, UserManageCommand,
};
pub use exchange::ExchangeSpec;
pub use fulltext::{FulltextManageCommand, FulltextSpec};
pub use graph::GraphSpec;
pub use join::{BuildSide, JoinSpec};
pub use recursive::RecursiveFragmentSpec;
pub use set::SetSpec;
pub use sink::{CopyTarget, SinkSpec};
pub use source::{BoundIndexPredicate, IndexProjection, SourceSpec};
pub use txn::TxnSpec;
pub use unary::UnarySpec;
pub use vector::{SpecVectorFilter, VectorManageCommand, VectorSpec};
