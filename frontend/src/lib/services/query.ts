import { post } from '$utils/http';
import type { QueryResult, QueryError } from '$types/query';

export interface ExecuteQueryParams {
  query: string;
  space?: string;
  sessionId?: string;
}

export interface ExecuteQueryResponse {
  success: boolean;
  data?: QueryResult;
  error?: QueryError;
  executionTime?: number;
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

export const queryService = {
  execute: async (params: ExecuteQueryParams): Promise<ExecuteQueryResponse> => {
    try {
      const startTime = Date.now();
      const body: Record<string, unknown> = { query: params.query };
      if (params.space !== undefined) body.space = params.space;
      if (params.sessionId !== undefined) body.sessionId = params.sessionId;
      const response = await post<QueryEnvelope>('/v1/query', body);
      const executionTime = response.metadata?.execution_time_ms ?? (Date.now() - startTime);
      if (!response.success) {
        return {
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
        success: true,
        data: { columns, rows, rowCount },
        executionTime,
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

  executeBatch: async (queries: string[], sessionId?: string): Promise<ExecuteQueryResponse[]> => {
    const results: ExecuteQueryResponse[] = [];
    for (const query of queries) {
      const result = await queryService.execute({ query, sessionId });
      results.push(result);
      if (!result.success) break;
    }
    return results;
  },
};

export default queryService;
