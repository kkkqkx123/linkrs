//! Search operation implementations
//!
//! Implements 5 search operators:
//! - FulltextSearch, FulltextLookup, MatchFulltext (fulltext operations)
//! - VectorSearch, VectorLookup (vector operations)

use super::super::super::chunk::DataChunk;
use crate::core::error::QueryError;
use crate::core::Value;
use super::super::StreamingExecutor;



// ============ FulltextSearch ============

pub fn open_fulltext_search(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::FulltextSearch { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_fulltext_search".to_string())),
    }
}

pub fn next_fulltext_search(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::FulltextSearch {
            input,
            search_query,
            space_id,
            tag_name,
            field_name,
            opened,
            #[cfg(feature = "fulltext-search")]
            fulltext_manager,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("FulltextSearch not opened".to_string()));
            }

            #[cfg(feature = "fulltext-search")]
            {
                if let Some(manager) = fulltext_manager {
                    let search_results = futures::executor::block_on(
                        manager.search(*space_id, tag_name, field_name, search_query, 100)
                    ).map_err(|e| QueryError::execution(format!("Fulltext search failed: {}", e)))?;
                    let mut rows = Vec::new();
                    for result in search_results {
                        rows.push(vec![
                            result.doc_id,
                            Value::Double(result.score as f64),
                        ]);
                    }
                    return if rows.is_empty() {
                        Ok(None)
                    } else {
                        Ok(Some(DataChunk::from_rows(rows)))
                    };
                }
            }

            // Fallback: if no fulltext manager, try to delegate to input
            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_fulltext_search".to_string())),
    }
}

pub fn stop_fulltext_search(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::FulltextSearch { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_fulltext_search".to_string())),
    }
}

pub fn close_fulltext_search(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::FulltextSearch { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_fulltext_search".to_string())),
    }
}

// ============ FulltextLookup ============

pub fn open_fulltext_lookup(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::FulltextLookup { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_fulltext_lookup".to_string())),
    }
}

pub fn next_fulltext_lookup(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::FulltextLookup {
            input,
            search_query,
            space_id,
            tag_name,
            field_name,
            opened,
            #[cfg(feature = "fulltext-search")]
            fulltext_manager,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("FulltextLookup not opened".to_string()));
            }

            #[cfg(feature = "fulltext-search")]
            {
                if let Some(manager) = fulltext_manager {
                    let search_results = futures::executor::block_on(
                        manager.search(*space_id, tag_name, field_name, search_query, 100)
                    ).map_err(|e| QueryError::execution(format!("Fulltext lookup failed: {}", e)))?;
                    let mut rows = Vec::new();
                    for result in search_results {
                        rows.push(vec![
                            result.doc_id,
                            Value::Double(result.score as f64),
                        ]);
                    }
                    return if rows.is_empty() {
                        Ok(None)
                    } else {
                        Ok(Some(DataChunk::from_rows(rows)))
                    };
                }
            }

            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_fulltext_lookup".to_string())),
    }
}

pub fn stop_fulltext_lookup(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::FulltextLookup { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_fulltext_lookup".to_string())),
    }
}

pub fn close_fulltext_lookup(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::FulltextLookup { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_fulltext_lookup".to_string())),
    }
}

// ============ MatchFulltext ============

pub fn open_match_fulltext(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::MatchFulltext { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_match_fulltext".to_string())),
    }
}

pub fn next_match_fulltext(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::MatchFulltext { input, opened, .. } => {
            if !*opened {
                return Err(QueryError::execution("MatchFulltext not opened".to_string()));
            }
            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_match_fulltext".to_string())),
    }
}

pub fn stop_match_fulltext(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::MatchFulltext { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_match_fulltext".to_string())),
    }
}

pub fn close_match_fulltext(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::MatchFulltext { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_match_fulltext".to_string())),
    }
}

// ============ VectorSearch ============

pub fn open_vector_search(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::VectorSearch { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_vector_search".to_string())),
    }
}

pub fn next_vector_search(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::VectorSearch { input, opened, .. } => {
            if !*opened {
                return Err(QueryError::execution("VectorSearch not opened".to_string()));
            }
            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_vector_search".to_string())),
    }
}

pub fn stop_vector_search(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::VectorSearch { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_vector_search".to_string())),
    }
}

pub fn close_vector_search(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::VectorSearch { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_vector_search".to_string())),
    }
}

// ============ VectorLookup ============

pub fn open_vector_lookup(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::VectorLookup { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_vector_lookup".to_string())),
    }
}

pub fn next_vector_lookup(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::VectorLookup { input, opened, .. } => {
            if !*opened {
                return Err(QueryError::execution("VectorLookup not opened".to_string()));
            }
            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_vector_lookup".to_string())),
    }
}

pub fn stop_vector_lookup(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::VectorLookup { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_vector_lookup".to_string())),
    }
}

pub fn close_vector_lookup(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::VectorLookup { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_vector_lookup".to_string())),
    }
}

// ============ VectorMatch ============

pub fn open_vector_match(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::VectorMatch { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_vector_match".to_string())),
    }
}

pub fn next_vector_match(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::VectorMatch { input, opened, .. } => {
            if !*opened {
                return Err(QueryError::execution("VectorMatch not opened".to_string()));
            }
            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_vector_match".to_string())),
    }
}

pub fn stop_vector_match(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::VectorMatch { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_vector_match".to_string())),
    }
}

pub fn close_vector_match(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::VectorMatch { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_vector_match".to_string())),
    }
}
