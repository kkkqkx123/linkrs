import { get } from '$utils/http';
import type { Vertex, Edge, VertexListParams, EdgeListParams } from '$types/data';
import type { ApiResponse_PaginatedResponse_Value } from '$types/schema';

export const dataService = {
  vertices: {
    list: async (spaceName: string, tagName: string, params?: VertexListParams): Promise<Vertex[]> =>
      await get<ApiResponse_PaginatedResponse_Value>(
        `/api/v1/data/spaces/${spaceName}/tags/${tagName}/vertices`,
        params
      ).then(res => res.data?.items || []),
  },
  edges: {
    list: async (spaceName: string, edgeName: string, params?: EdgeListParams): Promise<Edge[]> =>
      await get<ApiResponse_PaginatedResponse_Value>(
        `/api/v1/data/spaces/${spaceName}/edge-types/${edgeName}/edges`,
        params
      ).then(res => res.data?.items || []),
  },
};

export default dataService;
