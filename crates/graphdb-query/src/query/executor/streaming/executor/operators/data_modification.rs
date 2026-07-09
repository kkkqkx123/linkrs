//! Data modification operator implementations
//!
//! Includes: InsertVertices, InsertEdges, UpdateVertices, UpdateEdges,
//! DeleteVertices, DeleteEdges, PipeDeleteVertices, PipeDeleteEdges

use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::streaming::chunk::DataChunk;
use super::super::StreamingExecutor;

// ============ InsertVertices ============

pub fn open_insertvertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::InsertVertices { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_insertvertices".to_string())),
    }
}

pub fn next_insertvertices(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::InsertVertices {
            input,
            vertex_properties: _vertex_properties,
            tags: _tags,
            rows_inserted,
            opened,
        } => {
            if !*opened {
                return Err(QueryError::execution("InsertVertices not opened".to_string()));
            }

            // Process rows and count insertions
            if let Some(chunk) = input.next()? {
                let count = chunk.rows.len() as u64;
                *rows_inserted += count;
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_insertvertices".to_string())),
    }
}

pub fn stop_insertvertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::InsertVertices { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_insertvertices".to_string())),
    }
}

pub fn close_insertvertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::InsertVertices { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_insertvertices".to_string())),
    }
}

// ============ InsertEdges ============

pub fn open_insertedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::InsertEdges { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_insertedges".to_string())),
    }
}

pub fn next_insertedges(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::InsertEdges {
            input,
            src_col: _,
            dst_col: _,
            edge_type: _,
            edge_properties: _,
            rows_inserted,
            opened,
        } => {
            if !*opened {
                return Err(QueryError::execution("InsertEdges not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                let count = chunk.rows.len() as u64;
                *rows_inserted += count;
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_insertedges".to_string())),
    }
}

pub fn stop_insertedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::InsertEdges { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_insertedges".to_string())),
    }
}

pub fn close_insertedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::InsertEdges { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_insertedges".to_string())),
    }
}

// ============ UpdateVertices ============

pub fn open_updatevertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::UpdateVertices { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_updatevertices".to_string())),
    }
}

pub fn next_updatevertices(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::UpdateVertices {
            input,
            updates: _,
            rows_updated,
            opened,
        } => {
            if !*opened {
                return Err(QueryError::execution("UpdateVertices not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                let count = chunk.rows.len() as u64;
                *rows_updated += count;
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_updatevertices".to_string())),
    }
}

pub fn stop_updatevertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::UpdateVertices { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_updatevertices".to_string())),
    }
}

pub fn close_updatevertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::UpdateVertices { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_updatevertices".to_string())),
    }
}

// ============ UpdateEdges ============

pub fn open_updateedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::UpdateEdges { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_updateedges".to_string())),
    }
}

pub fn next_updateedges(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::UpdateEdges {
            input,
            updates: _,
            rows_updated,
            opened,
        } => {
            if !*opened {
                return Err(QueryError::execution("UpdateEdges not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                let count = chunk.rows.len() as u64;
                *rows_updated += count;
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_updateedges".to_string())),
    }
}

pub fn stop_updateedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::UpdateEdges { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_updateedges".to_string())),
    }
}

pub fn close_updateedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::UpdateEdges { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_updateedges".to_string())),
    }
}

// ============ DeleteVertices ============

pub fn open_deletevertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::DeleteVertices { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_deletevertices".to_string())),
    }
}

pub fn next_deletevertices(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::DeleteVertices {
            input,
            vertex_id_col: _,
            rows_deleted,
            opened,
        } => {
            if !*opened {
                return Err(QueryError::execution("DeleteVertices not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                let count = chunk.rows.len() as u64;
                *rows_deleted += count;
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_deletevertices".to_string())),
    }
}

pub fn stop_deletevertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::DeleteVertices { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_deletevertices".to_string())),
    }
}

pub fn close_deletevertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::DeleteVertices { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_deletevertices".to_string())),
    }
}

// ============ DeleteEdges ============

pub fn open_deleteedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::DeleteEdges { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_deleteedges".to_string())),
    }
}

pub fn next_deleteedges(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::DeleteEdges {
            input,
            src_col: _,
            dst_col: _,
            rows_deleted,
            opened,
        } => {
            if !*opened {
                return Err(QueryError::execution("DeleteEdges not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                let count = chunk.rows.len() as u64;
                *rows_deleted += count;
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_deleteedges".to_string())),
    }
}

pub fn stop_deleteedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::DeleteEdges { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_deleteedges".to_string())),
    }
}

pub fn close_deleteedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::DeleteEdges { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_deleteedges".to_string())),
    }
}

// ============ PipeDeleteVertices ============

pub fn open_pipedeletevertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::PipeDeleteVertices { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_pipedeletevertices".to_string())),
    }
}

pub fn next_pipedeletevertices(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::PipeDeleteVertices {
            input,
            vertex_id_col: _,
            rows_deleted,
            opened,
        } => {
            if !*opened {
                return Err(QueryError::execution("PipeDeleteVertices not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                let count = chunk.rows.len() as u64;
                *rows_deleted += count;
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_pipedeletevertices".to_string())),
    }
}

pub fn stop_pipedeletevertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::PipeDeleteVertices { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_pipedeletevertices".to_string())),
    }
}

pub fn close_pipedeletevertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::PipeDeleteVertices { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_pipedeletevertices".to_string())),
    }
}

// ============ PipeDeleteEdges ============

pub fn open_pipedeleteedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::PipeDeleteEdges { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_pipedeleteedges".to_string())),
    }
}

pub fn next_pipedeleteedges(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::PipeDeleteEdges {
            input,
            src_col: _,
            dst_col: _,
            rows_deleted,
            opened,
        } => {
            if !*opened {
                return Err(QueryError::execution("PipeDeleteEdges not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                let count = chunk.rows.len() as u64;
                *rows_deleted += count;
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_pipedeleteedges".to_string())),
    }
}

pub fn stop_pipedeleteedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::PipeDeleteEdges { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_pipedeleteedges".to_string())),
    }
}

pub fn close_pipedeleteedges(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::PipeDeleteEdges { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_pipedeleteedges".to_string())),
    }
}
