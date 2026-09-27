import { get } from '$utils/http';
import type { VertexListResponse, EdgeListResponse, FilterGroup, Statistics } from '$types/dataBrowser';
import type { ApiResponse_PaginatedResponse_Value } from '$types/schema';

export const dataBrowserService = {
  getVertices: async (
    space: string, tag: string, page: number, pageSize: number,
    sort: { field: string; order: 'asc' | 'desc' }, filters: any,
  ): Promise<VertexListResponse> => {
    const params: Record<string, string | number> = {
      limit: pageSize, offset: (page - 1) * pageSize,
      sort_by: sort.field, sort_order: sort.order.toUpperCase(),
    };
    if (filters && filters.conditions.length > 0) params.filter = JSON.stringify(filters);
    
    const res = await get<ApiResponse_PaginatedResponse_Value>(
      `/api/v1/data/spaces/${space}/tags/${tag}/vertices`,
      params
    );
    return {
      items: res.data?.items || [],
      total: res.data?.total || 0,
    } as VertexListResponse;
  },

  getEdges: async (
    space: string, type: string, page: number, pageSize: number,
    sort: { field: string; order: 'asc' | 'desc' }, filters: any,
  ): Promise<EdgeListResponse> => {
    const params: Record<string, string | number> = {
      limit: pageSize, offset: (page - 1) * pageSize,
      sort_by: sort.field, sort_order: sort.order.toUpperCase(),
    };
    if (filters && filters.conditions.length > 0) params.filter = JSON.stringify(filters);
    
    const res = await get<ApiResponse_PaginatedResponse_Value>(
      `/api/v1/data/spaces/${space}/edge-types/${type}/edges`,
      params
    );
    return {
      items: res.data?.items || [],
      total: res.data?.total || 0,
    } as EdgeListResponse;
  },

  getStatistics: async (space: string): Promise<Statistics> =>
    await get<Statistics>('/api/v1/schema/spaces/' + space + '/statistics'),
};
