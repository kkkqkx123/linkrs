//! Sampling Executor
//!
//! Implement the functionality of random sampling of query results, supporting various sampling methods.

use parking_lot::RwLock;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::collections::HashSet;
use std::sync::Arc;

use crate::core::error::{DBError, DBResult};
use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::base::InputExecutor;
use crate::query::executor::base::{BaseResultProcessor, ResultProcessor, ResultProcessorContext};
use crate::query::executor::base::{ExecutionResult, Executor};
use crate::query::DataSet;
use crate::storage::StorageClient;

/// Sampling method
#[derive(Debug, Clone, PartialEq)]
pub enum SampleMethod {
    /// Random sampling
    Random,
    /// Reservoir sampling (applicable to streaming data)
    Reservoir,
    /// System sampling (at fixed intervals)
    System,
}

/// SampleExecutor – A sampling executor
///
/// Implementation of a random sampling function for query results
pub struct SampleExecutor<S: StorageClient + Send + 'static> {
    /// Basic processor
    base: BaseResultProcessor<S>,
    /// Sampling Method
    method: SampleMethod,
    /// Number of samples
    count: usize,
    /// Random seed
    seed: Option<u64>,
    /// Input actuator
    input_executor: Option<Box<ExecutorEnum<S>>>,
}

impl<S: StorageClient + Send + 'static> SampleExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        method: SampleMethod,
        count: usize,
        seed: Option<u64>,
    ) -> Self {
        let base = BaseResultProcessor::new(
            id,
            "SampleExecutor".to_string(),
            "Samples query results using various sampling methods".to_string(),
            storage,
        );

        Self {
            base,
            method,
            count,
            seed,
            input_executor: None,
        }
    }

    fn process_input(&mut self) -> DBResult<ExecutionResult> {
        if let Some(ref mut input_exec) = self.input_executor {
            let input_result = input_exec.execute()?;
            self.sample_input(input_result)
        } else if let Some(input) = &self.base.input {
            self.sample_input(input.clone())
        } else {
            Err(DBError::query("Sample executor requires input".to_string()))
        }
    }

    /// Perform sampling on the input data.
    fn sample_input(&self, input: ExecutionResult) -> DBResult<ExecutionResult> {
        match input {
            ExecutionResult::DataSet(dataset) => {
                let sampled_dataset = self.sample_dataset(dataset)?;
                Ok(ExecutionResult::DataSet(sampled_dataset))
            }
            ExecutionResult::Empty
            | ExecutionResult::Success
            | ExecutionResult::SpaceSwitched(_) => Ok(input),
            ExecutionResult::Error(msg) => Err(DBError::query(msg)),
        }
    }

    /// Perform sampling on the dataset.
    fn sample_dataset(&self, dataset: DataSet) -> DBResult<DataSet> {
        match self.method {
            SampleMethod::Random => self.random_sample_dataset(dataset),
            SampleMethod::Reservoir => self.reservoir_sample_dataset(dataset),
            SampleMethod::System => self.system_sample_dataset(dataset),
        }
    }

    /// Randomly sampled dataset
    fn random_sample_dataset(&self, mut dataset: DataSet) -> DBResult<DataSet> {
        if dataset.rows.len() <= self.count {
            return Ok(dataset);
        }

        let mut rng = self.create_rng();
        let mut sampled_indices = HashSet::new();

        // Randomly select non-repeating indices.
        while sampled_indices.len() < self.count {
            let index = rng.gen_range(0..dataset.rows.len());
            sampled_indices.insert(index);
        }

        // Extract the rows of the sampling data.
        let sampled_rows: Vec<_> = sampled_indices
            .into_iter()
            .map(|i| dataset.rows[i].clone())
            .collect();

        dataset.rows = sampled_rows;
        Ok(dataset)
    }

    /// Reservoir sampling dataset
    fn reservoir_sample_dataset(&self, mut dataset: DataSet) -> DBResult<DataSet> {
        if dataset.rows.len() <= self.count {
            return Ok(dataset);
        }

        let mut rng = self.create_rng();
        let mut reservoir: Vec<_> = dataset.rows.iter().take(self.count).cloned().collect();

        // Process the remaining elements.
        for (i, row) in dataset.rows.iter().enumerate().skip(self.count) {
            let j = rng.gen_range(0..=i);
            if j < self.count {
                reservoir[j] = row.clone();
            }
        }

        dataset.rows = reservoir;
        Ok(dataset)
    }

    /// System-sampled dataset
    fn system_sample_dataset(&self, mut dataset: DataSet) -> DBResult<DataSet> {
        if dataset.rows.len() <= self.count {
            return Ok(dataset);
        }

        let step = dataset.rows.len() / self.count;
        let mut sampled_rows = Vec::new();

        for (i, row) in dataset.rows.iter().enumerate() {
            if i % step == 0 && sampled_rows.len() < self.count {
                sampled_rows.push(row.clone());
            }
        }

        dataset.rows = sampled_rows;
        Ok(dataset)
    }

    /// Create a random number generator
    fn create_rng(&self) -> StdRng {
        match self.seed {
            Some(seed) => StdRng::seed_from_u64(seed),
            None => StdRng::from_entropy(),
        }
    }
}

impl<S: StorageClient + Send + 'static> ResultProcessor for SampleExecutor<S> {
    fn process(&mut self, input: ExecutionResult) -> DBResult<ExecutionResult> {
        if self.input_executor.is_none() && self.base.input.is_none() {
            <Self as ResultProcessor>::set_input(self, input.clone());
        }
        self.process_input()
    }

    fn set_input(&mut self, input: ExecutionResult) {
        self.base.input = Some(input);
    }

    fn get_input(&self) -> Option<&ExecutionResult> {
        self.base.input.as_ref()
    }

    fn context(&self) -> &ResultProcessorContext {
        &self.base.context
    }

    fn set_context(&mut self, context: ResultProcessorContext) {
        self.base.context = context;
    }

    fn memory_usage(&self) -> usize {
        self.base.memory_usage
    }

    fn reset(&mut self) {
        self.base.reset_state();
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for SampleExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let input_result = if let Some(ref mut input_exec) = self.input_executor {
            input_exec.execute()?
        } else {
            self.base
                .input
                .clone()
                .unwrap_or(ExecutionResult::DataSet(crate::query::DataSet::new()))
        };

        self.process(input_result)
    }

    fn open(&mut self) -> DBResult<()> {
        if let Some(ref mut input_exec) = self.input_executor {
            input_exec.open()?;
        }
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
        if let Some(ref mut input_exec) = self.input_executor {
            input_exec.close()?;
        }
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.base.id > 0
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

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient + Send + 'static> InputExecutor<S> for SampleExecutor<S> {
    fn set_input(&mut self, input: ExecutorEnum<S>) {
        self.input_executor = Some(Box::new(input));
    }

    fn get_input(&self) -> Option<&ExecutorEnum<S>> {
        self.input_executor.as_deref()
    }
}

#[cfg(test)]
use crate::core::Value;
#[cfg(test)]
use crate::storage::MockStorage;
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sample_executor_random() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));

        // Create test data
        let values: Vec<Value> = (1..=100).map(Value::Int).collect();
        let dataset = DataSet::from_rows(
            values.into_iter().map(|v| vec![v]).collect(),
            vec!["_value".to_string()],
        );

        // Create a sampling executor that randomly selects 10 values, using a fixed seed to ensure reproducibility.
        let mut executor = SampleExecutor::new(1, storage, SampleMethod::Random, 10, Some(42));

        // Setting the input data
        <SampleExecutor<MockStorage> as ResultProcessor>::set_input(
            &mut executor,
            ExecutionResult::DataSet(dataset),
        );

        // Perform sampling
        let result = executor
            .process(ExecutionResult::DataSet(DataSet::new()))
            .expect("Failed to process sample");

        // Verification results
        match result {
            ExecutionResult::DataSet(sampled_dataset) => {
                assert_eq!(sampled_dataset.rows.len(), 10);
                for row in &sampled_dataset.rows {
                    match row[0] {
                        Value::Int(i) => {
                            assert!((1..=100).contains(&i));
                        }
                        _ => panic!("Expected Int values"),
                    }
                }
            }
            _ => panic!("Expected DataSet result"),
        }
    }

    #[test]
    fn test_sample_executor_reservoir() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));

        // Create test data
        let values: Vec<Value> = (1..=100).map(Value::Int).collect();
        let dataset = DataSet::from_rows(
            values.into_iter().map(|v| vec![v]).collect(),
            vec!["_value".to_string()],
        );

        // Create a sampling executor (sampling 5 values from a reservoir).
        let mut executor = SampleExecutor::new(1, storage, SampleMethod::Reservoir, 5, Some(123));

        // Set the input data
        <SampleExecutor<MockStorage> as ResultProcessor>::set_input(
            &mut executor,
            ExecutionResult::DataSet(dataset),
        );

        // Perform sampling
        let result = executor
            .process(ExecutionResult::DataSet(DataSet::new()))
            .expect("Failed to process sample");

        // Verification results
        match result {
            ExecutionResult::DataSet(sampled_dataset) => {
                assert_eq!(sampled_dataset.rows.len(), 5);
            }
            _ => panic!("Expected DataSet result"),
        }
    }
}
