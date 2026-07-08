//! Macros for reducing boilerplate code in transformation executors

/// Macro to implement HasStorage trait for executors
#[macro_export]
macro_rules! impl_has_storage {
    ($executor:ident) => {
        impl<S: $crate::storage::StorageClient + Send + 'static>
            $crate::query::executor::base::HasStorage<S> for $executor<S>
        {
            fn get_storage(&self) -> &std::sync::Arc<parking_lot::RwLock<S>> {
                self.base
                    .storage
                    .as_ref()
                    .expect(concat!(stringify!($executor), " storage should be set"))
            }
        }
    };
}

/// Macro to implement common Executor trait methods (without execute)
#[macro_export]
macro_rules! impl_executor_basic_methods {
    ($executor:ident) => {
        impl<S: $crate::storage::StorageClient + Send + Sync + 'static>
            $crate::query::executor::base::Executor<S> for $executor<S>
        {
            fn execute(
                &mut self,
            ) -> $crate::core::error::DBResult<$crate::query::executor::base::ExecutionResult> {
                unimplemented!("execute method must be implemented separately")
            }

            fn open(&mut self) -> $crate::core::error::DBResult<()> {
                Ok(())
            }

            fn close(&mut self) -> $crate::core::error::DBResult<()> {
                Ok(())
            }

            fn is_open(&self) -> bool {
                self.base.is_open()
            }

            fn id(&self) -> i64 {
                self.base.id
            }

            fn name(&self) -> &str {
                &self.base.name
            }

            fn description(&self) -> &str {
                &self.base.description
            }

            fn stats(&self) -> &$crate::query::executor::base::ExecutorStats {
                self.base.get_stats()
            }

            fn stats_mut(&mut self) -> &mut $crate::query::executor::base::ExecutorStats {
                self.base.get_stats_mut()
            }
        }
    };
}

/// Macro to implement complete Executor trait with execute method
#[macro_export]
macro_rules! impl_executor_with_execute {
    ($executor:ident, $execute_method:ident) => {
        impl<S: $crate::storage::StorageClient + Send + Sync + 'static>
            $crate::query::executor::base::Executor<S> for $executor<S>
        {
            fn execute(
                &mut self,
            ) -> $crate::core::error::DBResult<$crate::query::executor::base::ExecutionResult> {
                let dataset = self.$execute_method()?;
                Ok($crate::query::executor::base::ExecutionResult::DataSet(
                    dataset,
                ))
            }

            fn open(&mut self) -> $crate::core::error::DBResult<()> {
                Ok(())
            }

            fn close(&mut self) -> $crate::core::error::DBResult<()> {
                Ok(())
            }

            fn is_open(&self) -> bool {
                self.base.is_open()
            }

            fn id(&self) -> i64 {
                self.base.id
            }

            fn name(&self) -> &str {
                &self.base.name
            }

            fn description(&self) -> &str {
                &self.base.description
            }

            fn stats(&self) -> &$crate::query::executor::base::ExecutorStats {
                self.base.get_stats()
            }

            fn stats_mut(&mut self) -> &mut $crate::query::executor::base::ExecutorStats {
                self.base.get_stats_mut()
            }
        }
    };
}
