import { call, client } from '$lib/api/client';
import type { components } from '$types/schema.gen';

type BeginTransactionRequest = components['schemas']['BeginTransactionRequest'];
type TransactionResponse = components['schemas']['TransactionResponse'];

export interface BeginTransactionParams {
	readOnly?: boolean;
	timeoutSeconds?: number;
	queryTimeoutSeconds?: number;
	statementTimeoutSeconds?: number;
	idleTimeoutSeconds?: number;
}

export const transactionService = {
	begin: async (params?: BeginTransactionParams): Promise<TransactionResponse> => {
		const body: BeginTransactionRequest = {};
		if (params?.readOnly !== undefined) body.read_only = params.readOnly;
		if (params?.timeoutSeconds !== undefined) body.timeout_seconds = params.timeoutSeconds;
		if (params?.queryTimeoutSeconds !== undefined)
			body.query_timeout_seconds = params.queryTimeoutSeconds;
		if (params?.statementTimeoutSeconds !== undefined)
			body.statement_timeout_seconds = params.statementTimeoutSeconds;
		if (params?.idleTimeoutSeconds !== undefined) body.idle_timeout_seconds = params.idleTimeoutSeconds;
		return call(client.POST('/v1/transactions', { body }));
	},
	commit: async (id: number): Promise<unknown> =>
		call(client.POST('/v1/transactions/{id}/commit', { params: { path: { id } } })),
	rollback: async (id: number): Promise<unknown> =>
		call(client.POST('/v1/transactions/{id}/rollback', { params: { path: { id } } }))
};

export default transactionService;
