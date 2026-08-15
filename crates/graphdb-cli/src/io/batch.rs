use anyhow::Result;

use crate::session::manager::SessionManager;

pub struct BatchProcessor {
    buffer: Vec<String>,
    batch_size: usize,
}

impl BatchProcessor {
    pub fn new(batch_size: usize) -> Self {
        Self {
            buffer: Vec::new(),
            batch_size,
        }
    }

    pub fn add(&mut self, query: String) -> bool {
        self.buffer.push(query);
        self.buffer.len() >= self.batch_size
    }

    pub async fn flush(&mut self, session: &mut SessionManager) -> Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }

        let combined = self.buffer.join("; ");

        match session.execute_query(&combined).await {
            Ok(_) => {
                self.buffer.clear();
                Ok(())
            }
            Err(e) => Err(anyhow::anyhow!(
                "Batch of {} queries failed: {}",
                self.buffer.len(),
                e
            )),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_processor_add() {
        let mut processor = BatchProcessor::new(3);
        assert!(!processor.add("query1".to_string()));
        assert!(!processor.add("query2".to_string()));
        assert!(processor.add("query3".to_string()));
        assert_eq!(processor.len(), 3);
    }

    #[test]
    fn test_batch_processor_is_empty() {
        let mut processor = BatchProcessor::new(2);
        assert!(processor.is_empty());
        processor.add("query1".to_string());
        assert!(!processor.is_empty());
    }
}
