//! Management and DDL operation implementations
//!
//! Implements 7 management operators:
//! - SpaceManage, TagManage, EdgeManage, IndexManage (data management)
//! - UserManage, FulltextManage, VectorManage (system management)

use super::super::super::chunk::DataChunk;
use crate::core::error::QueryError;
use crate::core::Value;
use super::super::StreamingExecutor;

// ============ SpaceManage ============

pub fn open_space_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::SpaceManage { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_space_manage".to_string())),
    }
}

pub fn next_space_manage(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::SpaceManage { input, opened, .. } => {
            if !*opened {
                return Err(QueryError::execution("SpaceManage not opened".to_string()));
            }
            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_space_manage".to_string())),
    }
}

pub fn stop_space_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::SpaceManage { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_space_manage".to_string())),
    }
}

pub fn close_space_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::SpaceManage { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_space_manage".to_string())),
    }
}

// ============ TagManage ============

pub fn open_tag_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::TagManage { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_tag_manage".to_string())),
    }
}

pub fn next_tag_manage(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::TagManage { input, opened, .. } => {
            if !*opened {
                return Err(QueryError::execution("TagManage not opened".to_string()));
            }
            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_tag_manage".to_string())),
    }
}

pub fn stop_tag_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::TagManage { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_tag_manage".to_string())),
    }
}

pub fn close_tag_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::TagManage { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_tag_manage".to_string())),
    }
}

// ============ EdgeManage ============

pub fn open_edge_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::EdgeManage { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_edge_manage".to_string())),
    }
}

pub fn next_edge_manage(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::EdgeManage { input, opened, .. } => {
            if !*opened {
                return Err(QueryError::execution("EdgeManage not opened".to_string()));
            }
            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_edge_manage".to_string())),
    }
}

pub fn stop_edge_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::EdgeManage { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_edge_manage".to_string())),
    }
}

pub fn close_edge_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::EdgeManage { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_edge_manage".to_string())),
    }
}

// ============ IndexManage ============

pub fn open_index_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::IndexManage { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_index_manage".to_string())),
    }
}

pub fn next_index_manage(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::IndexManage { input, opened, .. } => {
            if !*opened {
                return Err(QueryError::execution("IndexManage not opened".to_string()));
            }
            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_index_manage".to_string())),
    }
}

pub fn stop_index_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::IndexManage { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_index_manage".to_string())),
    }
}

pub fn close_index_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::IndexManage { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_index_manage".to_string())),
    }
}

// ============ UserManage ============

pub fn open_user_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::UserManage { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_user_manage".to_string())),
    }
}

pub fn next_user_manage(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::UserManage { input, opened, .. } => {
            if !*opened {
                return Err(QueryError::execution("UserManage not opened".to_string()));
            }
            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_user_manage".to_string())),
    }
}

pub fn stop_user_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::UserManage { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_user_manage".to_string())),
    }
}

pub fn close_user_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::UserManage { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_user_manage".to_string())),
    }
}

// ============ FulltextManage ============

pub fn open_fulltext_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::FulltextManage { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_fulltext_manage".to_string())),
    }
}

pub fn next_fulltext_manage(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::FulltextManage { input, opened, .. } => {
            if !*opened {
                return Err(QueryError::execution("FulltextManage not opened".to_string()));
            }
            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_fulltext_manage".to_string())),
    }
}

pub fn stop_fulltext_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::FulltextManage { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_fulltext_manage".to_string())),
    }
}

pub fn close_fulltext_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::FulltextManage { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_fulltext_manage".to_string())),
    }
}

// ============ VectorManage ============

pub fn open_vector_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::VectorManage { input, opened, .. } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_vector_manage".to_string())),
    }
}

pub fn next_vector_manage(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::VectorManage { input, opened, .. } => {
            if !*opened {
                return Err(QueryError::execution("VectorManage not opened".to_string()));
            }
            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_vector_manage".to_string())),
    }
}

pub fn stop_vector_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::VectorManage { input, opened, .. } => {
            if *opened {
                input.stop()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_vector_manage".to_string())),
    }
}

pub fn close_vector_manage(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::VectorManage { input, opened, .. } => {
            if *opened {
                input.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_vector_manage".to_string())),
    }
}
