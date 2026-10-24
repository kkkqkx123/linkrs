import { call, client } from '$lib/api/client';

export interface CreateVectorIndexParams {
	space_id: number;
	tag_name: string;
	field_name: string;
	vector_size: number;
	distance?: string;
}

export interface VectorSearchParams {
	space_id: number;
	tag_name: string;
	field_name: string;
	query_vector: number[];
	limit?: number;
}

/**
 * Vector index management and search. Payloads stay opaque (`unknown`)
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

	search: async (params: VectorSearchParams): Promise<unknown> =>
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
