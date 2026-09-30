//! Primitive counter store: global, per-space and per-index read/write.
use std::collections::HashMap;
use std::sync::Arc;

use super::core::StatsManager;
use super::metric_type::MetricType;
use super::metric_value::MetricValue;

impl StatsManager {
    pub fn add_value(&self, metric_type: MetricType) {
        let metric = self
            .metrics
            .entry(metric_type)
            .or_insert_with(|| Arc::new(MetricValue::new(0)));
        metric.increment();
    }

    pub fn add_value_with_amount(&self, metric_type: MetricType, amount: u64) {
        let metric = self
            .metrics
            .entry(metric_type)
            .or_insert_with(|| Arc::new(MetricValue::new(0)));
        metric.add(amount);
    }

    pub fn dec_value(&self, metric_type: MetricType) {
        if let Some(metric) = self.metrics.get(&metric_type) {
            metric.decrement();
        }
    }

    pub fn set_value(&self, metric_type: MetricType, value: u64) {
        let metric = self
            .metrics
            .entry(metric_type)
            .or_insert_with(|| Arc::new(MetricValue::new(0)));
        metric.set(value);
    }

    pub fn add_space_metric(&self, space_name: &str, metric_type: MetricType) {
        let space_map = self
            .space_metrics
            .entry(space_name.to_string())
            .or_insert_with(|| Arc::new(dashmap::DashMap::new()));
        let metric = space_map
            .entry(metric_type)
            .or_insert_with(|| Arc::new(MetricValue::new(0)));
        metric.increment();
    }

    pub fn add_space_metric_with_amount(
        &self,
        space_name: &str,
        metric_type: MetricType,
        amount: u64,
    ) {
        let space_map = self
            .space_metrics
            .entry(space_name.to_string())
            .or_insert_with(|| Arc::new(dashmap::DashMap::new()));
        let metric = space_map
            .entry(metric_type)
            .or_insert_with(|| Arc::new(MetricValue::new(0)));
        metric.add(amount);
    }

    pub fn set_space_metric_with_amount(
        &self,
        space_name: &str,
        metric_type: MetricType,
        amount: u64,
    ) {
        let space_map = self
            .space_metrics
            .entry(space_name.to_string())
            .or_insert_with(|| Arc::new(dashmap::DashMap::new()));
        let metric = space_map
            .entry(metric_type)
            .or_insert_with(|| Arc::new(MetricValue::new(0)));
        metric.set(amount);
    }

    pub fn add_index_metric(&self, index_name: &str, metric_type: MetricType) {
        let index_map = self
            .index_metrics
            .entry(index_name.to_string())
            .or_insert_with(|| Arc::new(dashmap::DashMap::new()));
        let metric = index_map
            .entry(metric_type)
            .or_insert_with(|| Arc::new(MetricValue::new(0)));
        metric.increment();
    }

    pub fn add_index_metric_with_amount(
        &self,
        index_name: &str,
        metric_type: MetricType,
        amount: u64,
    ) {
        let index_map = self
            .index_metrics
            .entry(index_name.to_string())
            .or_insert_with(|| Arc::new(dashmap::DashMap::new()));
        let metric = index_map
            .entry(metric_type)
            .or_insert_with(|| Arc::new(MetricValue::new(0)));
        metric.add(amount);
    }

    pub fn dec_space_metric(&self, space_name: &str, metric_type: MetricType) {
        if let Some(space_map) = self.space_metrics.get(space_name) {
            if let Some(metric) = space_map.get(&metric_type) {
                metric.decrement();
            }
        }
    }

    pub fn get_value(&self, metric_type: MetricType) -> Option<u64> {
        self.metrics.get(&metric_type).map(|metric| metric.get())
    }

    pub fn get_space_value(&self, space_name: &str, metric_type: MetricType) -> Option<u64> {
        self.space_metrics
            .get(space_name)
            .and_then(|space_map| space_map.get(&metric_type).map(|metric| metric.get()))
    }

    pub fn get_index_value(&self, index_name: &str, metric_type: MetricType) -> Option<u64> {
        self.index_metrics
            .get(index_name)
            .and_then(|index_map| index_map.get(&metric_type).map(|metric| metric.get()))
    }

    pub fn get_all_index_metrics(&self, index_name: &str) -> Option<HashMap<MetricType, u64>> {
        self.index_metrics.get(index_name).map(|index_map| {
            index_map
                .iter()
                .map(|entry| (*entry.key(), entry.value().get()))
                .collect()
        })
    }

    pub fn get_all_metrics(&self) -> HashMap<MetricType, u64> {
        self.metrics
            .iter()
            .map(|entry| (*entry.key(), entry.value().get()))
            .collect()
    }

    pub fn get_all_space_metrics(&self, space_name: &str) -> Option<HashMap<MetricType, u64>> {
        self.space_metrics.get(space_name).map(|space_map| {
            space_map
                .iter()
                .map(|entry| (*entry.key(), entry.value().get()))
                .collect()
        })
    }

    pub fn reset_metric(&self, metric_type: MetricType) {
        if let Some(metric) = self.metrics.get(&metric_type) {
            metric.set(0);
        }
    }

    pub fn reset_all_metrics(&self) {
        for metric in self.metrics.iter() {
            metric.value().set(0);
        }
    }

    pub fn reset_space_metrics(&self, space_name: &str) {
        if let Some(space_map) = self.space_metrics.get(space_name) {
            for metric in space_map.iter() {
                metric.value().set(0);
            }
        }
    }
}
