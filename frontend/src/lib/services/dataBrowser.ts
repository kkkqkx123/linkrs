import { get } from '$utils/http';
import type { VertexListResponse, EdgeListResponse, FilterGroup, Statistics } from '$types/dataBrowser';

export const dataBrowserService = {
  getVertices: async (
    space: string, tag: string, page: number, pageSize: number,
    sort: { field: string; order: 'asc' | 'desc' }, filters: FilterGroup,
  ): Promise<VertexListResponse> => {
    const params: Record<string, string | number> = {
      limit: pageSize, offset: (page - 1) * pageSize,
      sort_by: sort.field, sort_order: sort.order.toUpperCase(),
    };
    if (filters && filters.conditions.length > 0) params.filter = JSON.stringify(filters);
    return await get(`/api/spaces/${space}/tags/${tag}/vertices`)(params) as VertexListResponse;
  },

  getEdges: async (
    space: string, type: string, page: number, pageSize: number,
    sort: { field: string; order: 'asc' | 'desc' }, filters: FilterGroup,
  ): Promise<EdgeListResponse> => {
    const params: Record<string, string | number> = {
      limit: pageSize, offset: (page - 1) * pageSize,
      sort_by: sort.field, sort_order: sort.order.toUpperCase(),
    };
    if (filters && filters.conditions.length > 0) params.filter = JSON.stringify(filters);
    return await get(`/api/spaces/${space}/edge-types/${type}/edges`)(params) as EdgeListResponse;
  },

  getStatistics: async (space: string): Promise<Statistics> =>
    await get('/api/data/statistics')({ space }) as Statistics,
};