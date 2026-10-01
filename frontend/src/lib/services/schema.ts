import { call, client, unwrap } from '$lib/api/client';
import type { Envelope } from '$lib/api/client';
import type { components } from '$types/schema.gen';
import type {
	Space,
	Tag,
	EdgeType,
	CreateTagParams,
	CreateEdgeTypeParams,
	CreateIndexParams
} from '$types/schema';

type Schemas = components['schemas'];
type SpaceStatistics = Schemas['SpaceStatistics'];
type SpaceDetailSchema = Schemas['SpaceDetail'];
type TagDetailSchema = Schemas['TagDetail'];
type EdgeTypeDetailSchema = Schemas['EdgeTypeDetail'];
type IndexInfoSchema = Schemas['IndexInfo'];
type CreateSpaceRequest = Schemas['CreateSpaceRequest'];
type CreateTagRequest = Schemas['CreateTagRequest'];
type CreateEdgeTypeRequest = Schemas['CreateEdgeTypeRequest'];
type CreateIndexRequest = Schemas['CreateIndexRequest'];

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null;
}

function asArray(value: unknown): Record<string, unknown>[] {
	return Array.isArray(value)
		? value.filter((item): item is Record<string, unknown> => isRecord(item))
		: [];
}

/** Pull a list out of the ad-hoc `{ key: [...] }` shapes of the bare endpoints. */
function pickList(payload: unknown, keys: string[]): Record<string, unknown>[] {
	if (Array.isArray(payload)) return asArray(payload);
	if (!isRecord(payload)) return [];
	if (isRecord(payload.data)) {
		for (const key of keys) {
			if (Array.isArray(payload.data[key])) return asArray(payload.data[key]);
		}
		if (Array.isArray((payload.data as Record<string, unknown>).items))
			return asArray((payload.data as Record<string, unknown>).items);
	}
	for (const key of keys) {
		if (Array.isArray(payload[key])) return asArray(payload[key]);
	}
	return [];
}

function toSpace(row: Record<string, unknown>): Space {
	return {
		id: typeof row.id === 'number' ? row.id : 0,
		name: typeof row.name === 'string' ? row.name : '',
		vid_type: typeof row.vid_type === 'string' ? row.vid_type : ''
	};
}

function toTag(row: Record<string, unknown>): Tag {
	return {
		id: typeof row.id === 'number' ? row.id : 0,
		name: typeof row.name === 'string' ? row.name : '',
		properties: Array.isArray(row.properties) ? (row.properties as Tag['properties']) : [],
		comment: typeof row.comment === 'string' ? row.comment : undefined,
		created_at: typeof row.created_at === 'number' ? row.created_at : 0
	};
}

function toEdgeType(row: Record<string, unknown>): EdgeType {
	return {
		id: typeof row.id === 'number' ? row.id : 0,
		name: typeof row.name === 'string' ? row.name : '',
		properties: Array.isArray(row.properties) ? (row.properties as EdgeType['properties']) : [],
		comment: typeof row.comment === 'string' ? row.comment : undefined,
		created_at: typeof row.created_at === 'number' ? row.created_at : 0
	};
}

function toIndexInfo(row: Record<string, unknown>): IndexInfoSchema {
	return {
		id: typeof row.id === 'number' ? row.id : 0,
		name: typeof row.name === 'string' ? row.name : '',
		index_type: typeof row.index_type === 'string' ? row.index_type : '',
		fields: Array.isArray(row.fields) ? row.fields.filter((f): f is string => typeof f === 'string') : [],
		status: typeof row.status === 'string' ? row.status : '',
		created_at: typeof row.created_at === 'number' ? row.created_at : 0,
		progress: typeof row.progress === 'number' ? row.progress : null
	};
}

export const schemaService = {
	spaces: {
		list: async (): Promise<Space[]> => {
			const payload = await call<unknown>(client.GET('/v1/schema/spaces'));
			return pickList(payload, ['spaces']).map(toSpace);
		},
		create: async (params: CreateSpaceRequest): Promise<{ message: string; space_name: string }> => {
			const payload = await call<unknown>(client.POST('/v1/schema/spaces', { body: params }));
			const record = isRecord(payload) ? payload : {};
			return {
				message: typeof record.message === 'string' ? record.message : '',
				space_name: typeof record.space_name === 'string' ? record.space_name : params.name
			};
		},
		get: async (name: string): Promise<{ space: { name: string; id: number } }> => {
			const payload = await call<unknown>(
				client.GET('/v1/schema/spaces/{name}', { params: { path: { name } } })
			);
			const record = isRecord(payload) && isRecord(payload.space) ? payload.space : {};
			return {
				space: {
					name: typeof record.name === 'string' ? record.name : name,
					id: typeof record.id === 'number' ? record.id : 0
				}
			};
		},
		getDetail: async (name: string): Promise<SpaceDetailSchema> =>
			unwrap(
				await call<Envelope<SpaceDetailSchema>>(
					client.GET('/api/v1/schema/spaces/{name}/details', { params: { path: { name } } })
				)
			),
		getStatistics: async (name: string): Promise<SpaceStatistics> =>
			unwrap(
				await call<Envelope<SpaceStatistics>>(
					client.GET('/api/v1/schema/spaces/{name}/statistics', { params: { path: { name } } })
				)
			),
		delete: async (name: string): Promise<{ message: string; space_name: string }> => {
			const payload = await call<unknown>(
				client.DELETE('/v1/schema/spaces/{name}', { params: { path: { name } } })
			);
			const record = isRecord(payload) ? payload : {};
			return {
				message: typeof record.message === 'string' ? record.message : '',
				space_name: typeof record.space_name === 'string' ? record.space_name : name
			};
		}
	},
	tags: {
		list: async (spaceName: string): Promise<Tag[]> => {
			const payload = await call<unknown>(
				client.GET('/v1/schema/spaces/{name}/tags', { params: { path: { name: spaceName } } })
			);
			return pickList(payload, ['tags']).map(toTag);
		},
		create: async (spaceName: string, params: CreateTagParams): Promise<unknown> => {
			const body: CreateTagRequest = { name: params.name, properties: params.properties };
			return call(
				client.POST('/v1/schema/spaces/{name}/tags', {
					params: { path: { name: spaceName } },
					body
				})
			);
		},
		getDetail: async (spaceName: string, tagName: string): Promise<TagDetailSchema> =>
			unwrap(
				await call<Envelope<TagDetailSchema>>(
					client.GET('/api/v1/schema/spaces/{name}/tags/{tag_name}', {
						params: { path: { name: spaceName, tag_name: tagName } }
					})
				)
			),
		delete: async (spaceName: string, tagName: string): Promise<void> => {
			await call<unknown>(
				client.DELETE('/api/v1/schema/spaces/{name}/tags/{tag_name}', {
					params: { path: { name: spaceName, tag_name: tagName } }
				})
			);
		}
	},
	edgeTypes: {
		list: async (spaceName: string): Promise<EdgeType[]> => {
			const payload = await call<unknown>(
				client.GET('/v1/schema/spaces/{name}/edge-types', {
					params: { path: { name: spaceName } }
				})
			);
			return pickList(payload, ['edge_types']).map(toEdgeType);
		},
		create: async (spaceName: string, params: CreateEdgeTypeParams): Promise<unknown> => {
			const body: CreateEdgeTypeRequest = { name: params.name, properties: params.properties };
			return call(
				client.POST('/v1/schema/spaces/{name}/edge-types', {
					params: { path: { name: spaceName } },
					body
				})
			);
		},
		getDetail: async (spaceName: string, edgeName: string): Promise<EdgeTypeDetailSchema> =>
			unwrap(
				await call<Envelope<EdgeTypeDetailSchema>>(
					client.GET('/api/v1/schema/spaces/{name}/edge-types/{edge_name}', {
						params: { path: { name: spaceName, edge_name: edgeName } }
					})
				)
			),
		delete: async (spaceName: string, edgeName: string): Promise<void> => {
			await call<unknown>(
				client.DELETE('/api/v1/schema/spaces/{name}/edge-types/{edge_name}', {
					params: { path: { name: spaceName, edge_name: edgeName } }
				})
			);
		}
	},
	indexes: {
		list: async (spaceName: string): Promise<IndexInfoSchema[]> => {
			const payload = await call<unknown>(
				client.GET('/api/v1/schema/spaces/{name}/indexes', {
					params: { path: { name: spaceName } }
				})
			);
			const envelope = payload as Envelope<unknown>;
			const inner =
				isRecord(envelope) && typeof envelope.success === 'boolean'
					? unwrap(envelope as Envelope<unknown>)
					: payload;
			return pickList(inner, ['indexes']).map(toIndexInfo);
		},
		create: async (spaceName: string, params: CreateIndexParams): Promise<unknown> => {
			const body: CreateIndexRequest = {
				name: params.name,
				index_type: params.index_type,
				entity_type: params.entity_type,
				entity_name: params.entity_name,
				fields: params.fields,
				comment: params.comment ?? null
			};
			return call(
				client.POST('/api/v1/schema/spaces/{name}/indexes', {
					params: { path: { name: spaceName } },
					body
				})
			);
		},
		getDetail: async (spaceName: string, indexName: string): Promise<IndexInfoSchema> =>
			unwrap(
				await call<Envelope<IndexInfoSchema>>(
					client.GET('/api/v1/schema/spaces/{name}/indexes/{index_name}', {
						params: { path: { name: spaceName, index_name: indexName } }
					})
				)
			),
		delete: async (spaceName: string, indexName: string): Promise<void> => {
			await call<unknown>(
				client.DELETE('/api/v1/schema/spaces/{name}/indexes/{index_name}', {
					params: { path: { name: spaceName, index_name: indexName } }
				})
			);
		},
		rebuild: async (spaceName: string, indexName: string): Promise<void> => {
			await call<unknown>(
				client.POST('/api/v1/schema/spaces/{name}/indexes/{index_name}/rebuild', {
					params: { path: { name: spaceName, index_name: indexName } }
				})
			);
		}
	}
};

export default schemaService;
