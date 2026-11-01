import { call, client, unwrap } from '$lib/api/client';
import type {
	Vertex,
	Edge,
	VertexListParams,
	EdgeListParams,
} from '$types/data';
import type { components } from '$lib/api/schema';

type VertexEnvelope = components['schemas']['ApiResponse_VertexPage'];
type EdgeEnvelope = components['schemas']['ApiResponse_EdgePage'];
type CellValue = components['schemas']['CellValue'];

function asRecord(value: CellValue | undefined): Record<string, CellValue> | null {
	if (typeof value === 'object' && value !== null && !Array.isArray(value)) {
		return value as Record<string, CellValue>;
	}
	return null;
}

function toVertex(item: { [key: string]: CellValue }): Vertex {
	const inner = asRecord(item['vertex']) ?? item;
	const record = inner as Record<string, CellValue>;
	const vid = record['vid'] ?? record['id'];
	const tags = asRecord(record['tags']);
	if (tags) {
		return { vid: String(vid ?? ''), tags: tags as unknown as Vertex['tags'] };
	}
	const tag = record['tag'];
	const properties = asRecord(record['properties']);
	if (typeof tag === 'string') {
		return {
			vid: String(vid ?? ''),
			tags: { [tag]: (properties ?? {}) as unknown as Record<string, unknown> },
		};
	}
	return { vid: String(vid ?? ''), tags: {} };
}

function toEdge(item: { [key: string]: CellValue }): Edge {
	const inner = asRecord(item['edge']) ?? item;
	const record = inner as Record<string, CellValue>;
	const src = record['src'];
	const dst = record['dst'];
	const edgeType = record['edge_type'] ?? record['type'];
	const rank = record['rank'];
	const properties = asRecord(record['properties']);
	return {
		src: typeof src === 'string' || typeof src === 'number' ? src : String(src ?? ''),
		dst: typeof dst === 'string' || typeof dst === 'number' ? dst : String(dst ?? ''),
		edge_type: typeof edgeType === 'string' ? edgeType : 'unknown',
		rank: typeof rank === 'number' ? rank : 0,
		properties: (properties ?? {}) as unknown as Record<string, unknown>,
	};
}

export const dataService = {
	vertices: {
		list: async (
			spaceName: string,
			tagName: string,
			params?: VertexListParams,
		): Promise<Vertex[]> => {
			const page = await unwrap(
				await call<VertexEnvelope>(
					client.GET('/api/v1/data/spaces/{name}/tags/{tag_name}/vertices', {
						params: {
							path: { name: spaceName, tag_name: tagName },
							query: {
								limit: params?.limit ?? 100,
								offset: params?.offset ?? 0,
								filter: params?.filter,
								sort_by: params?.sort_by,
								sort_order: params?.sort_order,
							},
						},
					}),
				),
			);
			return (page.items ?? []).map(toVertex);
		},
	},
	edges: {
		list: async (
			spaceName: string,
			edgeName: string,
			params?: EdgeListParams,
		): Promise<Edge[]> => {
			const page = await unwrap(
				await call<EdgeEnvelope>(
					client.GET(
						'/api/v1/data/spaces/{name}/edge-types/{edge_name}/edges',
						{
							params: {
								path: { name: spaceName, edge_name: edgeName },
								query: {
									limit: params?.limit ?? 100,
									offset: params?.offset ?? 0,
									filter: params?.filter,
									sort_by: params?.sort_by,
									sort_order: params?.sort_order,
								},
							},
						},
					),
				),
			);
			return (page.items ?? []).map(toEdge);
		},
	},
};

export default dataService;
