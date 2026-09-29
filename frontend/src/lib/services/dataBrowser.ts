import { get } from '$utils/http';
import { compileFilter } from '$utils/filterExpression';
import type { VertexListResponse, EdgeListResponse, FilterGroup, Statistics, VertexData, EdgeData } from '$types/dataBrowser';
import type { ApiResponse_PaginatedResponse_Value } from '$types/schema';

export const dataBrowserService = {
  getVertices: async (
    space: string, tag: string, page: number, pageSize: number,
    sort: { field: string; order: 'asc' | 'desc' }, filters: FilterGroup,
  ): Promise<VertexListResponse> => {
    const params: Record<string, string | number> = {
      limit: pageSize, offset: (page - 1) * pageSize,
      sort_by: sort.field, sort_order: sort.order.toUpperCase(),
    };
    const filterExpression = compileFilter(filters);
    if (filterExpression) params.filter = filterExpression;

    const res = await get<ApiResponse_PaginatedResponse_Value>(
      `/api/v1/data/spaces/${space}/tags/${tag}/vertices`,
      params
    );
    const data = (res.data ?? {}) as { items?: unknown[]; total?: number };
    return {
      data: (data.items || []) as VertexData[],
      total: data.total || 0,
      page,
      pageSize,
    } as VertexListResponse;
  },

  getEdges: async (
    space: string, type: string, page: number, pageSize: number,
    sort: { field: string; order: 'asc' | 'desc' }, filters: FilterGroup,
  ): Promise<EdgeListResponse> => {
    const params: Record<string, string | number> = {
      limit: pageSize, offset: (page - 1) * pageSize,
      sort_by: sort.field, sort_order: sort.order.toUpperCase(),
    };
    const filterExpression = compileFilter(filters);
    if (filterExpression) params.filter = filterExpression;

    const res = await get<ApiResponse_PaginatedResponse_Value>(
      `/api/v1/data/spaces/${space}/edge-types/${type}/edges`,
      params
    );
    const data = (res.data ?? {}) as { items?: unknown[]; total?: number };
    return {
      data: (data.items || []) as EdgeData[],
      total: data.total || 0,
      page,
      pageSize,
    } as EdgeListResponse;
  },

  getStatistics: async (space: string): Promise<Statistics> =>
    await get<Statistics>('/api/v1/schema/spaces/' + space + '/statistics'),
};
