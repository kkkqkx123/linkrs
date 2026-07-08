import { post } from '@/utils/http';

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

export interface CommitTransactionResponse {
  message: string;
  transaction_id: number;
}

export interface RollbackTransactionParams {
  session_id: number;
}

export interface RollbackTransactionResponse {
  message: string;
  transaction_id: number;
}

export const transactionService = {
  begin: async (params: BeginTransactionParams): Promise<BeginTransactionResponse> => {
    const response = await post('/v1/transactions')(params) as BeginTransactionResponse;
    return response;
  },

  commit: async (id: number, params: CommitTransactionParams): Promise<CommitTransactionResponse> => {
    const response = await post(`/v1/transactions/${id}/commit`)(params) as CommitTransactionResponse;
    return response;
  },

  rollback: async (id: number, params: RollbackTransactionParams): Promise<RollbackTransactionResponse> => {
    const response = await post(`/v1/transactions/${id}/rollback`)(params) as RollbackTransactionResponse;
    return response;
  },
};

export default transactionService;
