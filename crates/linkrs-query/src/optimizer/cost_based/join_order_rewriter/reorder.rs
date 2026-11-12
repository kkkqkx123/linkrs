mod logical;
mod physical;
mod shared;

pub use logical::walk_and_optimize_joins_logical;
pub use physical::walk_and_optimize_joins;

#[cfg(test)]
mod tests;
