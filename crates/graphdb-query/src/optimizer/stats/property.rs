//! Attribute Statistics Information Module
//!
//! Provide statistical information at the attribute level, which is used by the query optimizer to estimate selectivity.

use std::time::Instant;

use graphdb_core::value::Value;

use super::histogram::Histogram;

/// Property combination statistics
///
/// Lightweight attribute combination statistics for GROUP BY base estimation
#[derive(Debug, Clone)]
pub struct PropertyCombinationStats {
    /// Property key combinations (e.g. "tag.prop1.prop2")
    pub key: String,
    /// Associated tags (if any)
    pub tag_name: Option<String>,
    /// Property List
    pub properties: Vec<String>,
    /// Number of joint dissimilar values
    pub combined_distinct_values: u64,
    /// sample size
    pub sample_count: u64,
    /// Last updated
    pub last_updated: Instant,
}

impl PropertyCombinationStats {
    /// Create new property combination statistics.
    pub fn new(key: String, tag_name: Option<String>, properties: Vec<String>) -> Self {
        Self {
            key,
            tag_name,
            properties,
            combined_distinct_values: 0,
            sample_count: 0,
            last_updated: Instant::now(),
        }
    }

    /// Update statistics with new sample data.
    pub fn update(&mut self, distinct_values: u64, sample_count: u64) {
        // Use exponential moving average for stability
        if self.sample_count == 0 {
            self.combined_distinct_values = distinct_values;
            self.sample_count = sample_count;
        } else {
            let alpha = 0.3; // Smoothing factor
            self.combined_distinct_values = ((1.0 - alpha) * self.combined_distinct_values as f64
                + alpha * distinct_values as f64)
                as u64;
            self.sample_count = self.sample_count.saturating_add(sample_count);
        }
        self.last_updated = Instant::now();
    }

    /// Check if statistics are stale (older than 1 hour).
    pub fn is_stale(&self) -> bool {
        self.last_updated.elapsed().as_secs() > 3600
    }

    /// Get the estimated cardinality.
    pub fn estimated_cardinality(&self) -> u64 {
        self.combined_distinct_values.max(1)
    }
}

/// Attribute statistics information
#[derive(Debug, Clone)]
pub struct PropertyStatistics {
    /// Attribute name
    pub property_name: String,
    /// Associated Tags (optional)
    pub tag_name: Option<String>,
    /// Number of different values
    pub distinct_values: u64,
    /// Observed minimum value in the sampled window (orderable types only).
    pub min_value: Option<Value>,
    /// Observed maximum value in the sampled window (orderable types only).
    pub max_value: Option<Value>,
    /// Optional histograms (enabled for attributes with a high cardinality)
    pub histogram: Option<Histogram>,
    /// Is it appropriate to use a histogram? (Histograms are not necessary for attributes with a low cardinality.)
    pub use_histogram: bool,
}

impl PropertyStatistics {
    /// Create new attribute statistics information.
    pub fn new(property_name: String, tag_name: Option<String>) -> Self {
        Self {
            property_name,
            tag_name,
            distinct_values: 0,
            min_value: None,
            max_value: None,
            histogram: None,
            use_histogram: false,
        }
    }

    /// Setting up a histogram
    pub fn with_histogram(mut self, histogram: Histogram) -> Self {
        self.histogram = Some(histogram);
        self.use_histogram = true;
        self
    }

    /// Determine whether to use a histogram.
    pub fn should_use_histogram(&self) -> bool {
        self.use_histogram && self.histogram.is_some()
    }

    /// Record one observed value into the min/max envelope.
    ///
    /// Only values with a total order across the column (numeric and string
    /// families) participate; mixed-type columns never compare across
    /// families, so the envelope keeps the first-seen family.
    pub fn observe_value(&mut self, value: &Value) {
        let Some(key) = order_key(value) else {
            return;
        };
        match self.min_value.as_ref().and_then(order_key) {
            None => self.min_value = Some(value.clone()),
            Some(cur) if cur.0 == key.0 && key.1 < cur.1 => self.min_value = Some(value.clone()),
            _ => {}
        }
        match self.max_value.as_ref().and_then(order_key) {
            None => self.max_value = Some(value.clone()),
            Some(cur) if cur.0 == key.0 && key.1 > cur.1 => self.max_value = Some(value.clone()),
            _ => {}
        }
    }
}

/// Type-family-tagged ordering key for min/max tracking.
///
/// The second component orders only within one family (`i` int family,
/// `f` float family, `s` string family).
fn order_key(value: &Value) -> Option<(char, String)> {
    match value {
        Value::Null(_) => None,
        Value::SmallInt(i) => Some(('i', format!("{:+020}", *i as i64))),
        Value::Int(i) => Some(('i', format!("{i:+020}"))),
        Value::BigInt(i) => Some(('i', format!("{i:+020}"))),
        Value::Float(f) => Some(('f', ordered_bits(f.to_bits() as u64, 32))),
        Value::Double(d) => Some(('f', ordered_bits(d.to_bits(), 64))),
        Value::String(s) => Some(('s', s.to_string())),
        Value::FixedString(s) => Some(('s', s.clone())),
        _ => None,
    }
}

/// Order-preserving hex encoding of an IEEE-754 bit pattern: sign-flipped so
/// that lexicographic order on the encoding matches numeric order.
fn ordered_bits(bits: u64, width: usize) -> String {
    let flipped = if bits >> (width - 1) & 1 == 0 {
        bits | (1 << (width - 1))
    } else {
        !bits
    };
    format!("{flipped:0width$x}", width = width / 4)
}

impl Default for PropertyStatistics {
    fn default() -> Self {
        Self::new(String::new(), None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_value_tracks_numeric_envelope() {
        let mut stat = PropertyStatistics::new("age".to_string(), Some("person".to_string()));
        for v in [
            Value::Int(30),
            Value::Int(5),
            Value::Int(42),
            Value::Int(17),
        ] {
            stat.observe_value(&v);
        }
        assert_eq!(stat.min_value, Some(Value::Int(5)));
        assert_eq!(stat.max_value, Some(Value::Int(42)));
    }

    #[test]
    fn observe_value_ignores_null_and_keeps_first_family() {
        let mut stat = PropertyStatistics::new("v".to_string(), None);
        stat.observe_value(&Value::Null(graphdb_core::value::NullType::Null));
        assert_eq!(stat.min_value, None);
        stat.observe_value(&Value::Double(1.5));
        // A different family does not replace an existing envelope.
        stat.observe_value(&Value::Int(100));
        assert_eq!(stat.min_value, Some(Value::Double(1.5)));
    }

    #[test]
    fn observe_value_orders_strings_lexicographically() {
        let mut stat = PropertyStatistics::new("name".to_string(), None);
        for s in ["mango", "apple", "zebra"] {
            stat.observe_value(&Value::string(s));
        }
        assert_eq!(stat.min_value, Some(Value::string("apple")));
        assert_eq!(stat.max_value, Some(Value::string("zebra")));
    }
}
