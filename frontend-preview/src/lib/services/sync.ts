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

/**
 * Sync/outbox management service. All calls go through the shared
 * openapi-fetch client so session injection and auth redirects stay
 * in one place. Payloads are opaque (`unknown`) because the server
 * returns free-form JSON for diagnostics-style endpoints.
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
