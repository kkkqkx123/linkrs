import { call, client } from '$lib/api/client';
import { resolveSessionId } from '$utils/http';
import type { QueryResult, QueryError } from '$types/query';
import type { components } from '$lib/api/schema';
import { splitQueries } from '$utils/gql';
import { t } from '$i18n';

type QueryRequest = components['schemas']['QueryRequest'];
type QueryResponse = components['schemas']['QueryResponse'];
type BatchQueryRequest = components['schemas']['BatchQueryRequest'];
type ValidateRequest = components['schemas']['ValidateRequest'];
type ExplainRequest = components['schemas']['ExplainRequest'];

export interface ExecuteQueryParams {
	query: string;
	sessionId?: number;
	parameters?: Record<string, unknown>;
	sessionVariables?: Record<string, unknown>;
	signal?: AbortSignal;
	timeout?: number;
}

export interface BatchExecuteOptions {
	sessionId?: number;
	parameters?: Record<string, unknown>;
	sessionVariables?: Record<string, unknown>;
	signal?: AbortSignal;
	timeout?: number;
}

export interface ExecuteQueryResponse {
	success: boolean;
	data?: QueryResult;
	error?: QueryError;
	executionTime?: number;
	traceId?: string;
	stages?: components['schemas']['QueryStageTimings'];
	planNodeCount?: number;
}

/** Per-statement result inside a batch response. */
export interface BatchStatementResult {
	query: string;
	success: boolean;
	data?: QueryResult;
	error?: QueryError;
	executionTime?: number;
	truncated?: boolean;
	traceId?: string;
	stages?: components['schemas']['QueryStageTimings'];
	planNodeCount?: number;
}

/** Aggregated outcome of running a multi-statement script. */
export interface BatchExecuteResponse {
	results: BatchStatementResult[];
	totalExecutionTime: number;
	success: boolean;
}

/** Convert one utterance of the wire envelope into a typed result. */
function toStatementResult(
	query: string,
	response: QueryResponse,
	fallbackMs: number,
): BatchStatementResult {
	const executionTime = response.metadata?.execution_time_ms ?? fallbackMs;
	const traceId = response.metadata?.trace_id ?? undefined;
	const stages = response.metadata?.stages ?? undefined;
	const planNodeCount = response.metadata?.plan_node_count ?? undefined;
	if (!response.success) {
		const position = response.error?.position;
		return {
			query,
			success: false,
			error: {
				code: response.error?.code || 'EXECUTION_ERROR',
				message: response.error?.message || t('errors.executeQuery'),
				...(position != null
					? { position: { line: position.line, column: position.column } }
					: {}),
			},
			executionTime,
			traceId,
			stages,
			planNodeCount,
		};
	}
	const columns = response.data?.columns ?? [];
	const rows = (response.data?.rows ?? []) as Record<string, unknown>[];
	const rowCount = response.data?.row_count ?? rows.length;
	const truncated = response.metadata?.truncated === true;
	return {
		query,
		success: true,
		data: { columns, rows, rowCount, truncated },
		executionTime,
		truncated,
		traceId,
		stages,
		planNodeCount,
	};
}

/** Shared single-statement POST logic behind `execute` and `explain`. */
async function runSingle(
	endpoint: 'query' | 'explain',
	params: ExecuteQueryParams,
): Promise<ExecuteQueryResponse> {
	const resolved = resolveSessionId(params.sessionId);
	if (resolved === undefined) {
		return {
			success: false,
			error: { code: 'NO_SESSION', message: t('errors.missingSessionQuery') },
		};
	}
	if (!params.query.trim()) {
		return {
			success: false,
			error: { code: 'EMPTY_QUERY', message: t('errors.queryEmpty') },
		};
	}
	try {
		const startTime = Date.now();
		// The HTTP layer attaches `X-Session-ID` from storage for auth; the body
		// carries the statement text plus the session id required by the wire contract.
		const body: QueryRequest = { query: params.query, session_id: resolved };
		if (params.parameters !== undefined) body.parameters = params.parameters;
		if (params.sessionVariables !== undefined)
			body.session_variables = params.sessionVariables;
		const response =
			endpoint === 'query'
				? await call<QueryResponse>(
						client.POST('/v1/query', { body, signal: params.signal }),
					)
				: await call<QueryResponse>(
						client.POST('/v1/query/explain', {
							body: body as ExplainRequest,
							signal: params.signal,
						}),
					);
		const result = toStatementResult(
			params.query,
			response,
			Date.now() - startTime,
		);
		return {
			success: result.success,
			data: result.data,
			error: result.error,
			executionTime: result.executionTime,
			traceId: result.traceId,
			stages: result.stages,
			planNodeCount: result.planNodeCount,
		};
	} catch (error) {
		if (error instanceof DOMException && error.name === 'AbortError') {
			return {
				success: false,
				error: { code: 'QUERY_CANCELLED', message: t('errors.queryCancelled') },
			};
		}
		return {
			success: false,
			error: {
				code: 'EXECUTION_ERROR',
				message:
					error instanceof Error ? error.message : t('errors.executeQuery'),
			},
		};
	}
}

export const queryService = {
	execute: async (
		params: ExecuteQueryParams,
	): Promise<ExecuteQueryResponse> => runSingle('query', params),

	/**
	 * Run several auto-commit statements through the server-side batch window,
	 * which shares one commit boundary across all statements. The editor's raw
	 * script is split client-side so blank lines and comments are dropped before
	 * the statements leave the browser.
	 */
	executeBatch: async (
		script: string,
		options?: BatchExecuteOptions,
	): Promise<BatchExecuteResponse> => {
		const startTime = Date.now();
		const statements = splitQueries(script);
		if (statements.length === 0) {
			return { results: [], totalExecutionTime: 0, success: false };
		}
		const resolved = resolveSessionId(options?.sessionId);
		if (resolved === undefined) {
			return {
				results: statements.map((query) => ({
					query,
					success: false,
					error: {
						code: 'NO_SESSION',
						message: t('errors.missingSessionBatch'),
					},
					executionTime: 0,
				})),
				totalExecutionTime: 0,
				success: false,
			};
		}
		try {
			const batchBody: BatchQueryRequest = {
				session_id: resolved,
				statements,
			};
			if (options?.parameters !== undefined)
				batchBody.parameters = options.parameters;
			if (options?.sessionVariables !== undefined)
				batchBody.session_variables = options.sessionVariables;
			const response = await call<components['schemas']['BatchQueryResponse']>(
				client.POST('/v1/query/batch', { body: batchBody, signal: options?.signal }),
			);
			const envelopes = response.results ?? [];
			const fallbackMs = Math.round(
				(Date.now() - startTime) / statements.length,
			);
			const results = statements.map((query, index) => {
				const envelope = envelopes[index];
				if (!envelope) {
					return {
						query,
						success: false,
						error: {
							code: 'MISSING_RESULT',
							message: t('errors.missingResult'),
						},
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
			const message =
				error instanceof Error ? error.message : t('errors.executeBatch');
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
	 * syntax/semantic problems before the user commits to a run. The server
	 * also attaches an advisory row estimate used for automatic routing.
	 */
	validate: async (
		query: string,
		sessionId?: number,
		needEstimate?: boolean,
	): Promise<{
		valid: boolean;
		message: string;
		estimatedRows: number | null;
	}> => {
		const resolved = resolveSessionId(sessionId);
		if (resolved === undefined) {
			return {
				valid: false,
				message: t('errors.missingSessionValidate'),
				estimatedRows: null,
			};
		}
		try {
			const body: ValidateRequest = {
				query,
				session_id: resolved,
				need_estimate: needEstimate === true,
			};
			const response = await call<components['schemas']['ValidateResponse']>(
				client.POST('/v1/query/validate', { body }),
			);
			const estimated = response.estimated_rows;
			return {
				valid: response.valid,
				message: response.message,
				estimatedRows:
					typeof estimated === 'number' && Number.isFinite(estimated)
						? estimated
						: null,
			};
		} catch (error) {
			return {
				valid: false,
				message:
					error instanceof Error ? error.message : t('errors.validateQuery'),
				estimatedRows: null,
			};
		}
	},

	/**
	 * Plan a statement via EXPLAIN without executing it. Returns the plan
	 * rows on success, or a positioned error on failure.
	 */
	explain: async (
		params: ExecuteQueryParams,
	): Promise<ExecuteQueryResponse> => runSingle('explain', params),
};

export default queryService;
