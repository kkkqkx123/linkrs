import { post } from "@/utils/http";
import type { QueryResult, QueryError } from "@/types/query";

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

export const queryService = {
  // Execute a single query
  execute: async (
    params: ExecuteQueryParams,
  ): Promise<ExecuteQueryResponse> => {
    try {
      const startTime = Date.now();

      const response = (await post("/v1/query")(
        params,
      )) as ExecuteQueryResponse;

      const executionTime = Date.now() - startTime;

      return {
        ...response,
        executionTime: response.executionTime || executionTime,
      };
    } catch (error) {
      return {
        success: false,
        error: {
          code: "EXECUTION_ERROR",
          message:
            error instanceof Error ? error.message : "Failed to execute query",
        },
      };
    }
  },

  // Execute multiple queries sequentially
  executeBatch: async (
    queries: string[],
    sessionId?: string,
  ): Promise<ExecuteQueryResponse[]> => {
    const results: ExecuteQueryResponse[] = [];

    for (const query of queries) {
      const result = await queryService.execute({ query, sessionId });
      results.push(result);

      // Stop on error
      if (!result.success) {
        break;
      }
    }

    return results;
  },
};

export default queryService;
