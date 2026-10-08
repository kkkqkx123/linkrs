use std::collections::HashMap;

use linkrs_api::api_core::QueryResult;

use crate::query::parser::ast::Stmt;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

use super::error::GraphServiceError;
use super::GraphService;

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + Clone
            + crate::storage::AutoCommitBatchOps
            + 'static,
    > GraphService<S>
{
    /// Execute a batch of auto-commit DML statements inside one shared
    /// auto-commit batch window.
    pub async fn execute_batch(
        &self,
        session_id: i64,
        statements: &[String],
        parameters: Option<HashMap<String, linkrs_core::Value>>,
        session_variables: Option<HashMap<String, linkrs_core::Value>>,
    ) -> Vec<Result<QueryResult, GraphServiceError>> {
        let Some(session) = self.session_manager.find_session(session_id) else {
            return statements
                .iter()
                .map(|_| {
                    Err(GraphServiceError::new(format!(
                        "Invalid session ID: {session_id}"
                    )))
                })
                .collect();
        };
        session.charge();

        let mut results: Vec<Option<Result<QueryResult, GraphServiceError>>> =
            vec![None; statements.len()];
        let mut batch_indices: Vec<usize> = Vec::new();
        let mut batch_statements: Vec<String> = Vec::new();
        for (index, stmt) in statements.iter().enumerate() {
            match Self::parse_command(stmt) {
                Err(parse_error) => results[index] = Some(Err(parse_error)),
                Ok(Some(parsed)) => match parsed.ast.stmt() {
                    Stmt::AssignVariable(_) => {
                        results[index] = Some(
                            self.execute_with_params(
                                session_id,
                                stmt,
                                parameters.clone(),
                                session_variables.clone(),
                            )
                            .await,
                        );
                    }
                    _ => {
                        results[index] = Some(Err(GraphServiceError::new(
                            "Transaction commands are not supported in batch execution",
                        )));
                    }
                },
                Ok(None) => {
                    batch_indices.push(index);
                    batch_statements.push(stmt.clone());
                }
            }
        }

        if session.current_transaction().is_some() || !session.is_auto_commit() {
            for (index, stmt) in batch_statements.iter().enumerate() {
                results[batch_indices[index]] = Some(
                    self.execute_with_params(
                        session_id,
                        stmt,
                        parameters.clone(),
                        session_variables.clone(),
                    )
                    .await,
                );
            }
            return finalize_batch_outcomes(results);
        }

        let space_id = session.space().map(|s| s.id as i64).unwrap_or(0);
        let username = session.user();
        let (permitted, denied) = self.partition_permitted(&username, space_id, &batch_statements);

        let query_request = linkrs_api::api_core::QueryRequest {
            isolation_level: None,
            space_id: session.space().map(|s| s.id),
            space_name: session.space().map(|s| s.name),
            auto_commit: true,
            transaction_id: None,
            parameters,
            session_variables,
            query_id: None,
            parsed_statement: None,
            consistency: Default::default(),
        };
        let outcomes = self
            .query_api
            .write()
            .execute_batch(&permitted, query_request);

        merge_batch_outcomes(&mut results, &batch_indices, &denied, outcomes);
        finalize_batch_outcomes(results)
    }

    /// Split batch statements into permitted and denied groups.
    fn partition_permitted(
        &self,
        username: &str,
        space_id: i64,
        batch_statements: &[String],
    ) -> (Vec<String>, Vec<(usize, String)>) {
        use super::context::QueryExecutionContext;
        let mut denied: Vec<(usize, String)> = Vec::new();
        let mut permitted: Vec<String> = Vec::with_capacity(batch_statements.len());
        for (index, stmt) in batch_statements.iter().enumerate() {
            let context = QueryExecutionContext {
                stmt,
                parsed_ast: None,
                space_id,
                parameters: None,
                session_variables: None,
            };
            if let Err(e) = self.check_query_permission(username, &context) {
                denied.push((index, e.message().to_string()));
                continue;
            }
            permitted.push(stmt.clone());
        }
        (permitted, denied)
    }
}

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + Clone
            + crate::storage::AutoCommitBatchOps
            + crate::storage::AutoCommitGroupOps
            + 'static,
    > GraphService<S>
{
    /// Execute a batch of auto-commit DML statements inside shared
    /// group-commit windows.
    pub async fn execute_batch_grouped(
        &self,
        session_id: i64,
        statements: &[String],
        group_size: usize,
    ) -> Vec<Result<QueryResult, GraphServiceError>> {
        let Some(session) = self.session_manager.find_session(session_id) else {
            return statements
                .iter()
                .map(|_| {
                    Err(GraphServiceError::new(format!(
                        "Invalid session ID: {session_id}"
                    )))
                })
                .collect();
        };
        session.charge();

        let mut results: Vec<Option<Result<QueryResult, GraphServiceError>>> =
            vec![None; statements.len()];
        let mut batch_indices: Vec<usize> = Vec::new();
        let mut batch_statements: Vec<String> = Vec::new();
        for (index, stmt) in statements.iter().enumerate() {
            match Self::parse_command(stmt) {
                Err(parse_error) => results[index] = Some(Err(parse_error)),
                Ok(Some(parsed)) => match parsed.ast.stmt() {
                    Stmt::AssignVariable(_) => {
                        results[index] = Some(self.execute(session_id, stmt).await);
                    }
                    _ => {
                        results[index] = Some(Err(GraphServiceError::new(
                            "Transaction commands are not supported in batch execution",
                        )));
                    }
                },
                Ok(None) => {
                    batch_indices.push(index);
                    batch_statements.push(stmt.clone());
                }
            }
        }

        if session.current_transaction().is_some() || !session.is_auto_commit() {
            for (index, stmt) in batch_statements.iter().enumerate() {
                results[batch_indices[index]] = Some(self.execute(session_id, stmt).await);
            }
            return finalize_batch_outcomes(results);
        }

        let space_id = session.space().map(|s| s.id as i64).unwrap_or(0);
        let username = session.user();
        let (permitted, denied) = self.partition_permitted(&username, space_id, &batch_statements);

        let query_request = linkrs_api::api_core::QueryRequest {
            isolation_level: None,
            space_id: session.space().map(|s| s.id),
            space_name: session.space().map(|s| s.name),
            auto_commit: true,
            transaction_id: None,
            parameters: None,
            session_variables: None,
            query_id: None,
            parsed_statement: None,
            consistency: Default::default(),
        };
        let outcomes =
            self.query_api
                .write()
                .execute_batch_grouped(&permitted, query_request, group_size);

        merge_batch_outcomes(&mut results, &batch_indices, &denied, outcomes);
        finalize_batch_outcomes(results)
    }
}

/// Convert the per-slot batch results into the final ordered outcome vector.
fn finalize_batch_outcomes(
    results: Vec<Option<Result<QueryResult, GraphServiceError>>>,
) -> Vec<Result<QueryResult, GraphServiceError>> {
    results
        .into_iter()
        .map(|slot| slot.unwrap_or_else(|| Err(GraphServiceError::new("Batch outcome missing"))))
        .collect()
}

/// Merge the batch-window outcomes back into the per-slot results.
fn merge_batch_outcomes(
    results: &mut [Option<Result<QueryResult, GraphServiceError>>],
    batch_indices: &[usize],
    denied: &[(usize, String)],
    outcomes: Vec<Result<linkrs_api::api_core::QueryResult, linkrs_api::api_core::CoreError>>,
) {
    let mut permitted_outcomes = outcomes.into_iter();
    for (batch_pos, original_index) in batch_indices.iter().enumerate() {
        if let Some((_, error)) = denied.iter().find(|(i, _)| *i == batch_pos) {
            results[*original_index] = Some(Err(GraphServiceError::new(error.clone())));
            continue;
        }
        match permitted_outcomes.next() {
            Some(Ok(result)) => {
                results[*original_index] = Some(Ok(result));
            }
            Some(Err(error)) => {
                results[*original_index] = Some(Err(GraphServiceError::from_core_error(error)));
            }
            None => {
                results[*original_index] =
                    Some(Err(GraphServiceError::new("Batch outcome missing")));
            }
        }
    }
}
