use std::collections::HashMap;

use linkrs_api::api_core::QueryResult;

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
            + 'static,
    > GraphService<S>
{
    /// Best-effort output row estimate for automatic routing.
    pub async fn estimate_rows(&self, session_id: i64, stmt: &str) -> Option<u64> {
        let trimmed = stmt.trim();
        if trimmed.is_empty() {
            return None;
        }
        let explain = if trimmed.len() >= 7 && trimmed[..7].eq_ignore_ascii_case("explain") {
            trimmed.to_string()
        } else {
            format!("EXPLAIN {trimmed}")
        };
        let result = self
            .execute_with_params(session_id, &explain, None, None)
            .await
            .ok()?;
        let text = match result.first_value() {
            Some(value) => crate::value::to_json(value.clone()),
            None => return None,
        };
        Self::extract_root_estimate(text.as_str()?)
    }

    /// Plan a statement via `EXPLAIN` without executing it.
    pub async fn explain(
        &self,
        session_id: i64,
        stmt: &str,
        parameters: Option<HashMap<String, linkrs_core::Value>>,
        session_variables: Option<HashMap<String, linkrs_core::Value>>,
    ) -> Result<QueryResult, GraphServiceError> {
        let trimmed = stmt.trim();
        if trimmed.is_empty() {
            return Err(GraphServiceError::new("query must not be empty"));
        }
        let already_explain = trimmed.len() >= 7 && trimmed[..7].eq_ignore_ascii_case("explain");
        let explain_stmt = if already_explain {
            trimmed.to_string()
        } else {
            format!("EXPLAIN {trimmed}")
        };
        match self
            .execute_with_params(session_id, &explain_stmt, parameters, session_variables)
            .await
        {
            Ok(result) => Ok(result),
            Err(e) => {
                if already_explain {
                    return Err(e);
                }
                const PREFIX_LEN: usize = "EXPLAIN ".len();
                if let Some(pos) = e.position() {
                    if pos.line == 1 && pos.column > PREFIX_LEN {
                        return Err(GraphServiceError::with_position(
                            e.message().to_string(),
                            Some(linkrs_core::types::Position::new(
                                1,
                                pos.column - PREFIX_LEN,
                            )),
                        ));
                    }
                    if pos.line == 1 {
                        return Err(GraphServiceError::new(e.message().to_string()));
                    }
                }
                Err(e)
            }
        }
    }

    /// Scan an EXPLAIN table for `est_rows:<n>` markers and return the
    /// last one: the description lists producers before consumers, so the
    /// final marker belongs to the operator closest to the output.
    pub(crate) fn extract_root_estimate(plan_text: &str) -> Option<u64> {
        const MARKER: &str = "est_rows:";
        let mut estimate: Option<u64> = None;
        let mut rest = plan_text;
        while let Some(pos) = rest.find(MARKER) {
            rest = &rest[pos + MARKER.len()..];
            let len = rest
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .map(|c| c.len_utf8())
                .sum::<usize>();
            if len > 0 {
                if let Ok(parsed) = rest[..len].parse::<f64>() {
                    estimate = Some(parsed.round().max(0.0) as u64);
                }
                rest = &rest[len..];
            } else {
                match rest.chars().next() {
                    Some(c) => rest = &rest[c.len_utf8()..],
                    None => break,
                }
            }
        }
        estimate
    }
}
