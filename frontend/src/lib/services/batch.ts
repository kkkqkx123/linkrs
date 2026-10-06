import { call, client } from '$lib/api/client';

/**
 * Async batch-job management (`POST/GET /v1/batch*`). This is the job
 * API for bulk vertex/edge loads, distinct from the inline
 * `POST /v1/query/batch` multi-statement window used by the console.
 */
export const batchService = {
	create: async (params: {
		space_id: number;
		batch_type: string;
		batch_size?: number;
	}): Promise<unknown> =>
		call(
			client.POST('/v1/batch', {
				body: {
					space_id: params.space_id,
					batch_type: params.batch_type as never,
					batch_size: params.batch_size ?? 1000,
				} as never,
			}),
		),

	status: async (id: string): Promise<unknown> =>
		call(
			client.GET('/v1/batch/{id}', {
				params: { path: { id } },
			}),
		),

	addItems: async (id: string, items: unknown[]): Promise<unknown> =>
		call(
			client.POST('/v1/batch/{id}/items', {
				params: { path: { id } },
				body: { items } as never,
			}),
		),

	execute: async (id: string): Promise<unknown> =>
		call(
			client.POST('/v1/batch/{id}/execute', {
				params: { path: { id } },
			}),
		),

	cancel: async (id: string): Promise<unknown> =>
		call(
			client.POST('/v1/batch/{id}/cancel', {
				params: { path: { id } },
			}),
		),

	remove: async (id: string): Promise<unknown> =>
		call(
			client.DELETE('/v1/batch/{id}', {
				params: { path: { id } },
			}),
		),
};

export default batchService;
