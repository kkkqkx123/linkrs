import { call, client, unwrap } from '$lib/api/client';
import { compileFilter } from '$utils/filterExpression';
import type {
	VertexListResponse,
	EdgeListResponse,
	FilterGroup,
	Statistics,
	VertexData,
	EdgeData,
} from '$types/dataBrowser';
import type { components } from '$lib/api/schema';

type VertexPage = components['schemas']['VertexPage'];
type EdgePage = components['schemas']['EdgePage'];
type VertexEnvelope = components['schemas']['ApiResponse_VertexPage'];
type EdgeEnvelope = components['schemas']['ApiResponse_EdgePage'];
type SpaceStatistics = components['schemas']['SpaceStatistics'];
type CellValue = components['schemas']['CellValue'];

function asRecord(value: CellValue | undefined): Record<string, CellValue> | null {
	if (typeof value === 'object' && value !== null && !Array.isArray(value)) {
		return value as Record<string, CellValue>;
	}
	return null;
}

function toVertexData(item: { [key: string]: CellValue }): VertexData {
	const inner = asRecord(item['vertex']) ?? item;
	const record = inner as Record<string, CellValue>;
	const id = record['id'];
	const vid = record['vid'];
	const tag = record['tag'];
	const properties = record['properties'];
	return {
		id: typeof id === 'string' ? id : typeof vid === 'string' ? vid : String(id ?? vid ?? ''),
		tag: typeof tag === 'string' ? tag : 'unknown',
		properties:
			typeof properties === 'object' && properties !== null && !Array.isArray(properties)
				? (properties as unknown as Record<string, unknown>)
				: {},
	};
}

function toEdgeData(item: { [key: string]: CellValue }): EdgeData {
	const inner = asRecord(item['edge']) ?? item;
	const record = inner as Record<string, CellValue>;
	const id = record['id'];
	const type = record['type'] ?? record['edge_type'];
	const src = record['src'];
	const dst = record['dst'];
	const rank = record['rank'];
	const properties = record['properties'];
	return {
		id: typeof id === 'string' ? id : String(id ?? ''),
		type: typeof type === 'string' ? type : 'unknown',
		src: typeof src === 'string' ? src : String(src ?? ''),
		dst: typeof dst === 'string' ? dst : String(dst ?? ''),
		rank: typeof rank === 'number' ? rank : 0,
		properties:
			typeof properties === 'object' && properties !== null && !Array.isArray(properties)
				? (properties as unknown as Record<string, unknown>)
				: {},
	};
}

export const dataBrowserService = {
	getVertices: async (
		space: string,
		tag: string,
		page: number,
		pageSize: number,
		sort: { field: string; order: 'asc' | 'desc' },
		filters: FilterGroup,
	): Promise<VertexListResponse> => {
		const filterExpression = compileFilter(filters);
		const paged = await unwrap(
			await call<VertexEnvelope>(
				client.GET('/api/v1/data/spaces/{name}/tags/{tag_name}/vertices', {
					params: {
						path: { name: space, tag_name: tag },
						query: {
							limit: pageSize,
							offset: (page - 1) * pageSize,
							sort_by: sort.field,
							sort_order: sort.order.toUpperCase(),
							filter: filterExpression || undefined,
						},
					},
				}),
			),
		);
		const pageData: VertexPage = paged as VertexPage;
		return {
			data: (pageData.items ?? []).map(toVertexData),
			total: pageData.total ?? 0,
			page,
			pageSize,
		};
	},

	getEdges: async (
		space: string,
		type: string,
		page: number,
		pageSize: number,
		sort: { field: string; order: 'asc' | 'desc' },
		filters: FilterGroup,
	): Promise<EdgeListResponse> => {
		const filterExpression = compileFilter(filters);
		const paged = await unwrap(
			await call<EdgeEnvelope>(
				client.GET('/api/v1/data/spaces/{name}/edge-types/{edge_name}/edges', {
					params: {
						path: { name: space, edge_name: type },
						query: {
							limit: pageSize,
							offset: (page - 1) * pageSize,
							sort_by: sort.field,
							sort_order: sort.order.toUpperCase(),
							filter: filterExpression || undefined,
						},
					},
				}),
			),
		);
		const pageData: EdgePage = paged as EdgePage;
		return {
			data: (pageData.items ?? []).map(toEdgeData),
			total: pageData.total ?? 0,
			page,
			pageSize,
		};
	},

	getStatistics: async (space: string): Promise<Statistics> => {
		const stats = unwrap(
			await call<components['schemas']['ApiResponse_SpaceStatistics']>(
				client.GET('/api/v1/schema/spaces/{name}/statistics', {
					params: { path: { name: space } },
				}),
			),
		);
		return toStatistics(stats);
	},
};

/** Map the contract statistics onto the browser view model. */
function toStatistics(stats: SpaceStatistics): Statistics {
	return {
		totalVertices: stats.estimated_vertex_count ?? 0,
		totalEdges: stats.estimated_edge_count ?? 0,
		tagCount: stats.tag_count ?? 0,
		edgeTypeCount: stats.edge_type_count ?? 0,
		tagDistribution: [],
		edgeTypeDistribution: [],
	};
}

export default dataBrowserService;
