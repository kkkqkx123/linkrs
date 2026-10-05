import { call, client } from '$lib/api/client';
import type { components } from '$lib/api/schema';

export type OverviewResponse = components['schemas']['OverviewResponse'];
export type SystemResourceResponse =
	components['schemas']['SystemResourceResponse'];
export type DatabaseOverviewResponse =
	components['schemas']['DatabaseOverviewResponse'];
export type QueryStatsResponse = components['schemas']['QueryStatsResponse'];
export type SearchStatsResponse = components['schemas']['SearchStatsResponse'];
export type QueryProfileDetailResponse =
	components['schemas']['QueryProfileDetailResponse'];
export type QueryStageTimings = components['schemas']['QueryStageTimings'];
export type SyncStatusResponse = components['schemas']['SyncStatusResponse'];

/** Transaction metrics endpoint is untyped in the contract; keep it opaque. */
export type TransactionMetrics = Record<string, unknown>;

export interface QueryStatsParams {
	from?: string;
	to?: string;
}

/**
 * Statistics service grouped by backend resource. Every call goes through
 * the shared openapi-fetch client so session injection, auth redirects and
 * bigint parsing stay in one place. Space statistics and health checks keep
 * their existing services; this module only adds the monitoring endpoints.
 */
export const statisticsService = {
	overview: async (): Promise<OverviewResponse> =>
		call(client.GET('/v1/statistics/overview')),

	system: async (): Promise<SystemResourceResponse> =>
		call(client.GET('/v1/statistics/system')),

	database: async (): Promise<DatabaseOverviewResponse> =>
		call(client.GET('/v1/statistics/database')),

	queries: async (params?: QueryStatsParams): Promise<QueryStatsResponse> =>
		call(
			client.GET('/v1/statistics/queries', {
				params: { query: { from: params?.from, to: params?.to } },
			}),
		),

	queryProfile: async (traceId: string): Promise<QueryProfileDetailResponse> =>
		call(
			client.GET('/v1/statistics/queries/{trace_id}', {
				params: { path: { trace_id: traceId } },
			}),
		),

	search: async (): Promise<SearchStatsResponse> =>
		call(client.GET('/v1/statistics/search')),

	transactionMetrics: async (): Promise<TransactionMetrics> =>
		call(client.GET('/v1/transactions/metrics')),

	syncStatus: async (): Promise<SyncStatusResponse> =>
		call(client.GET('/v1/sync/status')),
};

export default statisticsService;
