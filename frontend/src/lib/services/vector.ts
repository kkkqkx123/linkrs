import { call, client } from '$lib/api/client';

export interface CreateVectorIndexParams {
	space_id: number;
	tag_name: string;
	field_name: string;
	vector_size: number;
	distance?: string;
}

/**
 * Vector index management. Payloads stay opaque (`unknown`)
 * because index info shapes vary by backend; the page renders them as
 * JSON plus the key result tables.
 */
export const vectorService = {
	list: async (): Promise<unknown> =>
		call(client.GET('/v1/vector/indexes')),

	info: async (
		spaceId: number,
		tagName: string,
		fieldName: string,
	): Promise<unknown> =>
		call(
			client.GET('/v1/vector/indexes/{space_id}/{tag_name}/{field_name}', {
				params: { path: { space_id: spaceId, tag_name: tagName, field_name: fieldName } },
			}),
		),

	create: async (params: CreateVectorIndexParams): Promise<unknown> =>
		call(
			client.POST('/v1/vector/indexes', {
				body: {
					space_id: params.space_id,
					tag_name: params.tag_name,
					field_name: params.field_name,
					vector_size: params.vector_size,
					distance: (params.distance ?? 'Cosine') as never,
				} as never,
			}),
		),

	drop: async (
		spaceId: number,
		tagName: string,
		fieldName: string,
	): Promise<unknown> =>
		call(
			client.DELETE('/v1/vector/indexes/{space_id}/{tag_name}/{field_name}', {
				params: { path: { space_id: spaceId, tag_name: tagName, field_name: fieldName } },
			}),
		),

	rebuild: async (params: {
		space_id: number;
		tag_name: string;
		field_name: string;
	}): Promise<unknown> =>
		call(
			client.POST('/v1/vector/indexes/rebuild', {
				body: { ...params } as never,
			}),
		),

	search: async (params: {
		space_id: number;
		tag_name: string;
		field_name: string;
		query_vector: number[];
		limit?: number;
	}): Promise<unknown> =>
		call(
			client.POST('/v1/vector/search', {
				body: {
					space_id: params.space_id,
					tag_name: params.tag_name,
					field_name: params.field_name,
					query_vector: params.query_vector,
					limit: params.limit ?? 10,
				} as never,
			}),
		),

	count: async (
		spaceId: number,
		tagName: string,
		fieldName: string,
	): Promise<unknown> =>
		call(
			client.GET('/v1/vector/{space_id}/{tag_name}/{field_name}/count', {
				params: { path: { space_id: spaceId, tag_name: tagName, field_name: fieldName } },
			}),
		),

	getPoint: async (
		spaceId: number,
		tagName: string,
		fieldName: string,
		pointId: string,
	): Promise<unknown> =>
		call(
			client.GET('/v1/vector/{space_id}/{tag_name}/{field_name}/{point_id}', {
				params: { path: { space_id: spaceId, tag_name: tagName, field_name: fieldName, point_id: pointId } },
			}),
		),

	scroll: async (params: {
		space_id: number;
		tag_name: string;
		field_name: string;
		limit?: number;
		offset?: string;
	}): Promise<unknown> =>
		call(
			client.POST('/v1/vector/scroll', {
				body: {
					space_id: params.space_id,
				tag_name: params.tag_name,
				field_name: params.field_name,
				limit: params.limit ?? 10,
					offset: params.offset ?? null,
				} as never,
			}),
		),

	setPayload: async (params: {
		space_id: number;
		tag_name: string;
		field_name: string;
		point_ids: string[];
		payload: Record<string, unknown>;
	}): Promise<unknown> =>
		call(
			client.PUT('/v1/vector/payload', {
				body: { ...params } as never,
			}),
		),

	mergePayloadFields: async (params: {
		space_id: number;
		tag_name: string;
		field_name: string;
		point_ids: string[];
		payload: Record<string, unknown>;
	}): Promise<unknown> =>
		call(
			client.PUT('/v1/vector/payload/fields', {
				body: { ...params } as never,
			}),
		),

	deletePayload: async (params: {
		space_id: number;
		tag_name: string;
		field_name: string;
		point_ids: string[];
		keys: string[];
	}): Promise<unknown> =>
		call(
			client.POST('/v1/vector/payload/delete', {
				body: { ...params } as never,
			}),
		),

	rebuildStatus: async (id: string): Promise<unknown> =>
		call(
			client.GET('/v1/vector/rebuilds/{id}', {
				params: { path: { id } },
			}),
		),

	clear: async (params: {
		space_id: number;
		tag_name: string;
		field_name: string;
	}): Promise<unknown> =>
		call(
			client.POST('/v1/vector/indexes/clear', {
				body: { ...params, force: true } as never,
			}),
		),
};

export default vectorService;
