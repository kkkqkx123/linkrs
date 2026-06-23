//! Data modification actuator module
//!
//! Contains all the actuators associated with data modification that modify data in the storage layer

pub mod delete;
pub mod index_ops;
pub mod insert;
pub mod remove;
pub mod tag_ops;
pub mod update;

pub use delete::{DeleteExecutor, PipeDeleteExecutor};
pub use index_ops::{CreateIndexExecutor, DropIndexExecutor};
pub use insert::InsertExecutor;
pub use remove::{RemoveExecutor, RemoveItem, RemoveItemType, RemoveResult};
pub use tag_ops::DeleteTagExecutor;
pub use update::{EdgeUpdate, UpdateExecutor, UpdateResult, VertexUpdate};
