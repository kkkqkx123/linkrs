//! Graph traversal operator implementations
//!
//! Includes: Expand, ExpandAll, Traverse, TraverseAll, AppendVertices,
//! BiExpand, BiTraverse, ShortestPath, BFSShortest, AllPaths, MultiShortestPath

use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::streaming::chunk::DataChunk;
use super::super::StreamingExecutor;

// ============ Expand ============

pub fn open_expand(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Expand {
            input,
            opened,
            ..
        } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_expand".to_string())),
    }
}

pub fn next_expand(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Expand {
            input,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("Expand not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_expand".to_string())),
    }
}

pub fn stop_expand(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Expand { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_expand".to_string())),
    }
}

pub fn close_expand(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Expand { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_expand".to_string())),
    }
}

// ============ ExpandAll ============

pub fn open_expandall(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::ExpandAll {
            input,
            opened,
            ..
        } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_expandall".to_string())),
    }
}

pub fn next_expandall(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::ExpandAll {
            input,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("ExpandAll not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_expandall".to_string())),
    }
}

pub fn stop_expandall(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::ExpandAll { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_expandall".to_string())),
    }
}

pub fn close_expandall(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::ExpandAll { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_expandall".to_string())),
    }
}

// ============ Traverse ============

pub fn open_traverse(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Traverse {
            input,
            opened,
            ..
        } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_traverse".to_string())),
    }
}

pub fn next_traverse(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::Traverse {
            input,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("Traverse not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_traverse".to_string())),
    }
}

pub fn stop_traverse(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Traverse { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_traverse".to_string())),
    }
}

pub fn close_traverse(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::Traverse { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_traverse".to_string())),
    }
}

// ============ TraverseAll ============

pub fn open_traverseall(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::TraverseAll {
            input,
            opened,
            ..
        } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_traverseall".to_string())),
    }
}

pub fn next_traverseall(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::TraverseAll {
            input,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("TraverseAll not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_traverseall".to_string())),
    }
}

pub fn stop_traverseall(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::TraverseAll { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_traverseall".to_string())),
    }
}

pub fn close_traverseall(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::TraverseAll { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_traverseall".to_string())),
    }
}

// ============ AppendVertices ============

pub fn open_appendvertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::AppendVertices {
            input,
            opened,
            ..
        } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_appendvertices".to_string())),
    }
}

pub fn next_appendvertices(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::AppendVertices {
            input,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("AppendVertices not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_appendvertices".to_string())),
    }
}

pub fn stop_appendvertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::AppendVertices { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_appendvertices".to_string())),
    }
}

pub fn close_appendvertices(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::AppendVertices { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_appendvertices".to_string())),
    }
}

// ============ BiExpand ============

pub fn open_biexpand(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BiExpand {
            input,
            opened,
            ..
        } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_biexpand".to_string())),
    }
}

pub fn next_biexpand(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::BiExpand {
            input,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("BiExpand not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_biexpand".to_string())),
    }
}

pub fn stop_biexpand(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BiExpand { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_biexpand".to_string())),
    }
}

pub fn close_biexpand(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BiExpand { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_biexpand".to_string())),
    }
}

// ============ BiTraverse ============

pub fn open_bitraverse(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BiTraverse {
            input,
            opened,
            ..
        } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_bitraverse".to_string())),
    }
}

pub fn next_bitraverse(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::BiTraverse {
            input,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("BiTraverse not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_bitraverse".to_string())),
    }
}

pub fn stop_bitraverse(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BiTraverse { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_bitraverse".to_string())),
    }
}

pub fn close_bitraverse(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BiTraverse { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_bitraverse".to_string())),
    }
}

// ============ ShortestPath ============

pub fn open_shortestpath(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::ShortestPath {
            input,
            opened,
            ..
        } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_shortestpath".to_string())),
    }
}

pub fn next_shortestpath(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::ShortestPath {
            input,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("ShortestPath not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_shortestpath".to_string())),
    }
}

pub fn stop_shortestpath(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::ShortestPath { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_shortestpath".to_string())),
    }
}

pub fn close_shortestpath(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::ShortestPath { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_shortestpath".to_string())),
    }
}

// ============ BFSShortest ============

pub fn open_bfsshortest(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BFSShortest {
            input,
            opened,
            ..
        } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_bfsshortest".to_string())),
    }
}

pub fn next_bfsshortest(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::BFSShortest {
            input,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("BFSShortest not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_bfsshortest".to_string())),
    }
}

pub fn stop_bfsshortest(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BFSShortest { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_bfsshortest".to_string())),
    }
}

pub fn close_bfsshortest(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::BFSShortest { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_bfsshortest".to_string())),
    }
}

// ============ AllPaths ============

pub fn open_allpaths(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::AllPaths {
            input,
            opened,
            ..
        } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_allpaths".to_string())),
    }
}

pub fn next_allpaths(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::AllPaths {
            input,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("AllPaths not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_allpaths".to_string())),
    }
}

pub fn stop_allpaths(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::AllPaths { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_allpaths".to_string())),
    }
}

pub fn close_allpaths(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::AllPaths { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_allpaths".to_string())),
    }
}

// ============ MultiShortestPath ============

pub fn open_multishortestpath(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::MultiShortestPath {
            input,
            opened,
            ..
        } => {
            input.open()?;
            *opened = true;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in open_multishortestpath".to_string())),
    }
}

pub fn next_multishortestpath(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::MultiShortestPath {
            input,
            opened,
            ..
        } => {
            if !*opened {
                return Err(QueryError::execution("MultiShortestPath not opened".to_string()));
            }

            if let Some(chunk) = input.next()? {
                return Ok(Some(chunk));
            }
            Ok(None)
        }
        _ => Err(QueryError::execution("Type mismatch in next_multishortestpath".to_string())),
    }
}

pub fn stop_multishortestpath(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::MultiShortestPath { input, .. } => {
            input.stop()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in stop_multishortestpath".to_string())),
    }
}

pub fn close_multishortestpath(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::MultiShortestPath { input, .. } => {
            input.close()?;
            Ok(())
        }
        _ => Err(QueryError::execution("Type mismatch in close_multishortestpath".to_string())),
    }
}
