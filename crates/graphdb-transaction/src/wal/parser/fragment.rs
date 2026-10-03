//! Fragment reassembly state machine for split WAL records.

use graphdb_core::wal::types::{RecordType, WalHeader};

/// Buffer for reassembling fragmented WAL records
#[derive(Default)]
pub(super) struct FragmentBuffer {
    /// Current fragments being assembled
    fragments: Vec<Vec<u8>>,
    /// Header of the first fragment
    first_header: Option<WalHeader>,
    /// Expected next record type
    expected_next: Option<RecordType>,
}

impl FragmentBuffer {
    pub(super) fn new() -> Self {
        Self {
            fragments: Vec::new(),
            first_header: None,
            expected_next: None,
        }
    }

    pub(super) fn reset(&mut self) {
        self.fragments.clear();
        self.first_header = None;
        self.expected_next = None;
    }

    pub(super) fn add_fragment(&mut self, header: WalHeader, payload: Vec<u8>) -> bool {
        let record_type = header.record_type;

        match record_type {
            RecordType::Full => {
                self.reset();
                true
            }
            RecordType::First => {
                self.reset();
                self.fragments.push(payload);
                self.first_header = Some(header);
                self.expected_next = Some(RecordType::Middle);
                false
            }
            RecordType::Middle => {
                if self.expected_next != Some(RecordType::Middle)
                    && self.expected_next != Some(RecordType::Last)
                {
                    self.reset();
                    return false;
                }
                self.fragments.push(payload);
                self.expected_next = Some(RecordType::Middle);
                false
            }
            RecordType::Last => {
                if self.expected_next != Some(RecordType::Middle)
                    && self.expected_next != Some(RecordType::Last)
                {
                    self.reset();
                    return false;
                }
                self.fragments.push(payload);
                true
            }
        }
    }

    pub(super) fn assemble(&self) -> Option<Vec<u8>> {
        if self.fragments.is_empty() {
            return None;
        }
        let mut result = Vec::new();
        for fragment in &self.fragments {
            result.extend_from_slice(fragment);
        }
        Some(result)
    }

    pub(super) fn get_first_header(&self) -> Option<&WalHeader> {
        self.first_header.as_ref()
    }
}
