import { call, client } from '$lib/api/client';

/**
 * Fulltext index maintenance. The server exposes rebuild/clear/status
 * plus the inconsistent-index listing; search itself runs through GQL,
 * so this service only covers the management endpoints.
 */
export const fulltextService = {
	indexes: async (): Promise<unknown> =>
		call(client.GET('/v1/fulltext/indexes')),

	search: async (params: {
		space_id: number;
		tag_name: string;
		field_name: string;
		query: string;
		limit?: number;
	}): Promise<unknown> =>
		call(
			client.POST('/v1/fulltext/search', {
				body: {
					space_id: params.space_id,
					tag_name: params.tag_name,
					field_name: params.field_name,
					query: params.query,
					limit: params.limit ?? 10,
				} as never,
			}),
		),

	inconsistent: async (): Promise<unknown> =>
		call(client.GET('/v1/fulltext/indexes/inconsistent')),

	rebuild: async (params: {
		space_id: number;
		tag_name: string;
		field_name: string;
	}): Promise<unknown> =>
		call(
			client.POST('/v1/fulltext/indexes/rebuild', {
				body: { ...params } as never,
			}),
		),

	rebuildStatus: async (id: string): Promise<unknown> =>
		call(
			client.GET('/v1/fulltext/rebuilds/{id}', {
				params: { path: { id } },
			}),
		),

	clear: async (params: {
		space_id: number;
		tag_name: string;
		field_name: string;
	}): Promise<unknown> =>
		call(
			client.POST('/v1/fulltext/indexes/clear', {
				body: { ...params, force: true } as never,
			}),
		),
};

export default fulltextService;
