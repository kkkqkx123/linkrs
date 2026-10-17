//! Table information used by the join order optimizer.

/// Table information
#[derive(Debug, Clone)]
pub struct TableInfo {
    /// Table identifier (variable name)
    pub id: String,
    /// Estimated number of lines
    pub estimated_rows: u64,
    /// Selective (0.0 ~ 1.0)
    pub selectivity: f64,
    /// Is there an index available?
    pub has_index: bool,
    /// The unique identifier of the table (used for bit operations)
    pub bit_id: u32,
}

impl TableInfo {
    /// Create information for a new table.
    pub fn new(id: String, estimated_rows: u64) -> Self {
        Self {
            id,
            estimated_rows,
            selectivity: 1.0,
            has_index: false,
            bit_id: 0,
        }
    }

    /// Set the option to be selective.
    pub fn with_selectivity(mut self, selectivity: f64) -> Self {
        self.selectivity = selectivity.clamp(0.0, 1.0);
        self
    }

    /// Set whether an index is to be created.
    pub fn with_index(mut self, has_index: bool) -> Self {
        self.has_index = has_index;
        self
    }

    /// Set Bit ID
    pub fn with_bit_id(mut self, bit_id: u32) -> Self {
        self.bit_id = bit_id;
        self
    }
}
