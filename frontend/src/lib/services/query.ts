import { post } from '$utils/http';
import type { QueryResult, QueryError } from '$types/query';
import { splitQueries } from '$utils/gql';

export interface ExecuteQueryParams {
  query: string;
  sessionId?: number;
}

export interface ExecuteQueryResponse {
  success: boolean;
  data?: QueryResult;
  error?: QueryError;
  executionTime?: number;
}

/** Per-statement result inside a batch response. */
export interface BatchStatementResult {
  query: string;
  success: boolean;
  data?: QueryResult;
  error?: QueryError;
  executionTime?: number;
}

/** Aggregated outcome of running a multi-statement script. */
export interface BatchExecuteResponse {
  results: BatchStatementResult[];
  totalExecutionTime: number;
  success: boolean;
}

interface QueryEnvelope {
  success: boolean;
  data?: {
    columns?: string[];
    rows?: Record<string, unknown>[];
    row_count?: number;
  };
  error?: { code?: string; message?: string };
  metadata?: { execution_time_ms?: number; rows_returned?: number };
}

interface BatchEnvelope {
  results?: QueryEnvelope[];
}

function resolveSessionId(explicit?: number): number | undefined {
  if (explicit !== undefined) return explicit;
  const stored = localStorage.getItem('sessionId');
  if (!stored) return undefined;
  const parsed = Number(stored);
  return Number.isFinite(parsed) ? parsed : undefined;
}

/** Convert one utterance of the wire envelope into a typed result. */
function toStatementResult(query: string, response: QueryEnvelope, fallbackMs: number): BatchStatementResult {
  const executionTime = response.metadata?.execution_time_ms ?? fallbackMs;
  if (!response.success) {
    return {
      query,
      success: false,
      error: {
        code: response.error?.code || 'EXECUTION_ERROR',
        message: response.error?.message || 'Failed to execute query',
      },
      executionTime,
    };
  }
  const columns = response.data?.columns ?? [];
  const rows = response.data?.rows ?? [];
  const rowCount = response.data?.row_count ?? rows.length;
  return {
    query,
    success: true,
    data: { columns, rows, rowCount },
    executionTime,
  };
}

export const queryService = {
  execute: async (params: ExecuteQueryParams): Promise<ExecuteQueryResponse> => {
    const resolved = resolveSessionId(params.sessionId);
    if (resolved === undefined) {
      return {
        success: false,
        error: { code: 'NO_SESSION', message: 'Missing session id for query execution' },
      };
    }
    try {
      const startTime = Date.now();
      // The HTTP layer attaches `X-Session-ID` from storage for auth; the body
      // carries the statement text plus the session id required by the wire contract.
      const body: Record<string, unknown> = { query: params.query, session_id: resolved };
      const response = await post<QueryEnvelope>('/v1/query', body);
      const result = toStatementResult(params.query, response, Date.now() - startTime);
      return {
        success: result.success,
        data: result.data,
        error: result.error,
        executionTime: result.executionTime,
      };
    } catch (error) {
      return {
        success: false,
        error: {
          code: 'EXECUTION_ERROR',
          message: error instanceof Error ? error.message : 'Failed to execute query',
        },
      };
    }
  },

  /**
   * Run several auto-commit statements through the server-side batch window,
   * which shares one commit boundary across all statements. The editor's raw
   * script is split client-side so blank lines and comments are dropped before
   * the statements leave the browser.
   */
  executeBatch: async (script: string, sessionId?: number): Promise<BatchExecuteResponse> => {
    const startTime = Date.now();
    const statements = splitQueries(script);
    if (statements.length === 0) {
      return { results: [], totalExecutionTime: 0, success: false };
    }
    const resolved = resolveSessionId(sessionId);
    if (resolved === undefined) {
      return {
        results: statements.map((query) => ({
          query,
          success: false,
          error: { code: 'NO_SESSION', message: 'Missing session id for batch execution' },
          executionTime: 0,
        })),
        totalExecutionTime: 0,
        success: false,
      };
    }
    try {
      const response = await post<BatchEnvelope>('/v1/query/batch', {
        session_id: resolved,
        statements,
      });
      const envelopes = response.results ?? [];
      const fallbackMs = Math.round((Date.now() - startTime) / statements.length);
      const results = statements.map((query, index) => {
        const envelope = envelopes[index];
        if (!envelope) {
          return {
            query,
            success: false,
            error: { code: 'MISSING_RESULT', message: 'Server returned no result for this statement' },
            executionTime: 0,
          };
        }
        return toStatementResult(query, envelope, fallbackMs);
      });
      return {
        results,
        totalExecutionTime: Date.now() - startTime,
        success: results.every((r) => r.success),
      };
    } catch (error) {
      const message = error instanceof Error ? error.message : 'Failed to execute batch';
      return {
        results: statements.map((query) => ({
          query,
          success: false,
          error: { code: 'EXECUTION_ERROR', message },
          executionTime: 0,
        })),
        totalExecutionTime: Date.now() - startTime,
        success: false,
      };
    }
  },

  /**
   * Parse and bind a statement without executing it, so the console can flag
   * syntax/semantic problems before the user commits to a run.
   */
  validate: async (query: string, sessionId?: number): Promise<{ valid: boolean; message: string }> => {
    const resolved = resolveSessionId(sessionId);
    if (resolved === undefined) {
      return { valid: false, message: 'Missing session id for validation' };
    }
    try {
      const response = await post<{ valid: boolean; message: string }>('/v1/query/validate', {
        query,
        session_id: resolved,
      });
      return { valid: response.valid, message: response.message };
    } catch (error) {
      return {
        valid: false,
        message: error instanceof Error ? error.message : 'Failed to validate query',
      };
    }
  },
};

export default queryService;
