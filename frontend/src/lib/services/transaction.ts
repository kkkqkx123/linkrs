import { post } from '$utils/http';
import type { BeginTransactionRequest, BatchQueryRequest, BatchQueryResponse } from '$types/schema';

export interface BeginTransactionParams {
  session_id: number;
  read_only?: boolean;
  timeout_seconds?: number;
  query_timeout_seconds?: number;
  statement_timeout_seconds?: number;
  idle_timeout_seconds?: number;
}

export interface BeginTransactionResponse {
  transaction_id: number;
  status: string;
}

export interface CommitTransactionParams {
  session_id: number;
}

export interface RollbackTransactionParams {
  session_id: number;
}

export const transactionService = {
  begin: async (params: BeginTransactionParams): Promise<BeginTransactionResponse> =>
    await post('/v1/transactions', params as any),
  commit: async (id: number, params: CommitTransactionParams): Promise<{ message: string; transaction_id: number }> =>
    await post(`/v1/transactions/${id}/commit`, params),
  rollback: async (id: number, params: RollbackTransactionParams): Promise<{ message: string; transaction_id: number }> =>
    await post(`/v1/transactions/${id}/rollback`, params),
};

export default transactionService;
