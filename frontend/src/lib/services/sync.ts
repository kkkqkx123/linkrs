import { call, client } from '$lib/api/client';
import type { components } from '$lib/api/schema';

export type SyncStatusResponse = components['schemas']['SyncStatusResponse'];

export interface DeadLettersParams {
	target?: string;
	index_id?: number;
	generation?: number;
	limit?: number;
	offset?: number;
}

export interface RequeueParams {
	target?: string;
	index_id?: number;
	generation?: number;
	limit?: number;
	event_ids?: number[];
}

export interface ClearDegradedParams {
	target: string;
	index_id: number;
	generation: number;
	start_lsn: number;
	end_lsn: number;
}

/** One dead-letter row. All fields optional: the server returns
 * free-form JSON and older backends may omit gauges. */
export interface DeadLetterRow {
	event_id?: unknown;
	target?: unknown;
	index_id?: unknown;
	generation?: unknown;
	lsn?: unknown;
	state?: unknown;
	attempts?: unknown;
	last_error?: unknown;
}

export interface DeadLettersResponse {
	dead_letters?: DeadLetterRow[];
}

/** One degraded-range row; fields mirror the server DTO. */
export interface DegradedRangeRow {
	target?: unknown;
	index_id?: unknown;
	generation?: unknown;
	start_lsn?: unknown;
	end_lsn?: unknown;
}

export interface DegradedRangesResponse {
	degraded_ranges?: DegradedRangeRow[];
}

/** Extract the row array from either a wrapped or bare payload. */
function rowsOf<T>(payload: unknown, key: string): T[] {
	if (payload && typeof payload === 'object') {
		const wrapped = (payload as Record<string, unknown>)[key];
		if (Array.isArray(wrapped)) return wrapped as T[];
	}
	if (Array.isArray(payload)) return payload as T[];
	return [];
}

/**
 * Sync/outbox management service. All calls go through the shared
 * openapi-fetch client so session injection and auth redirects stay
 * in one place. Diagnostics-style endpoints stay `unknown`; list
 * endpoints expose typed row helpers alongside the raw payload.
 */
export const syncService = {
	status: async (): Promise<SyncStatusResponse> =>
		call(client.GET('/v1/sync/status')),

	diagnostics: async (): Promise<unknown> =>
		call(client.GET('/v1/sync/outbox/diagnostics')),

	deadLetters: async (params?: DeadLettersParams): Promise<unknown> =>
		call(
			client.GET('/v1/sync/outbox/dead_letters', {
				params: {
					query: {
						target: params?.target,
						index_id: params?.index_id,
						generation: params?.generation,
						limit: params?.limit ?? 100,
						offset: params?.offset ?? 0,
					},
				},
			}),
		),

	deadLetterRows: async (params?: DeadLettersParams): Promise<DeadLetterRow[]> =>
		rowsOf<DeadLetterRow>(await syncService.deadLetters(params), 'dead_letters'),

	requeue: async (params?: RequeueParams): Promise<unknown> =>
		call(
			client.POST('/v1/sync/outbox/requeue', {
				body: {
					target: params?.target,
					index_id: params?.index_id,
					generation: params?.generation,
					limit: params?.limit ?? 100,
					event_ids: params?.event_ids,
				} as never,
			}),
		),

	retryOutbox: async (): Promise<unknown> =>
		call(client.POST('/v1/sync/outbox/retry')),

	degradedRangeRows: async (params?: {
		target?: string;
		index_id?: number;
		generation?: number;
	}): Promise<DegradedRangeRow[]> =>
		rowsOf<DegradedRangeRow>(await syncService.degradedRanges(params), 'degraded_ranges'),

	degradedRanges: async (params?: {
		target?: string;
		index_id?: number;
		generation?: number;
	}): Promise<unknown> =>
		call(
			client.GET('/v1/sync/outbox/degraded_ranges', {
				params: {
					query: {
						target: params?.target,
						index_id: params?.index_id,
						generation: params?.generation,
					},
				},
			}),
		),

	clearDegraded: async (params: ClearDegradedParams): Promise<unknown> =>
		call(
			client.POST('/v1/sync/outbox/degraded/clear', {
				body: { ...params } as never,
			}),
		),

	retentionRun: async (params?: {
		grace_lsn_distance?: number;
		max_age_ms?: number;
	}): Promise<unknown> =>
		call(
			client.POST('/v1/sync/outbox/retention/run', {
				body: { ...params } as never,
			}),
		),

	retentionStatus: async (): Promise<unknown> =>
		call(client.GET('/v1/sync/outbox/retention/status')),
};

export default syncService;
