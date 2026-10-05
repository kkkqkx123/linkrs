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

type Paginated = components['schemas']['ApiResponse_PaginatedResponse_Value'];
type SpaceStatistics = components['schemas']['SpaceStatistics'];

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
			await call<Paginated>(
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
		return {
			data: (paged.items || []) as VertexData[],
			total: paged.total || 0,
			page,
			pageSize,
		} as VertexListResponse;
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
			await call<Paginated>(
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
		return {
			data: (paged.items || []) as EdgeData[],
			total: paged.total || 0,
			page,
			pageSize,
		} as EdgeListResponse;
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
