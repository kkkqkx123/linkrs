use std::future::Future;

use crate::client::QueryResult;
use crate::session::manager::SessionManager;

pub trait StreamingExport {
    fn export_streaming(
        &self,
        query: &str,
        chunk_size: usize,
        session: &mut SessionManager,
    ) -> impl Future<Output = anyhow::Result<ExportStream>> + Send;
}

pub struct ExportStream {
    offset: usize,
    chunk_size: usize,
    query: String,
}

impl ExportStream {
    pub fn new(query: String, chunk_size: usize) -> Self {
        Self {
            offset: 0,
            chunk_size,
            query,
        }
    }

    pub async fn next_chunk(
        &mut self,
        session: &mut SessionManager,
    ) -> anyhow::Result<Option<QueryResult>> {
        let paginated_query = format!(
            "{} SKIP {} LIMIT {}",
            self.query, self.offset, self.chunk_size
        );

        match session.execute_query(&paginated_query).await {
            Ok(result) => {
                if result.rows.is_empty() {
                    Ok(None)
                } else {
                    self.offset += result.rows.len();
                    Ok(Some(result))
                }
            }
            Err(e) => Err(anyhow::anyhow!("Stream query failed: {}", e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_export_stream_creation() {
        let query = "SELECT * FROM user".to_string();
        let chunk_size = 100;

        let stream = ExportStream::new(query, chunk_size);
        assert_eq!(stream.offset, 0);
        assert_eq!(stream.chunk_size, 100);
    }
}
