import { get } from '@/utils/http';
import type { Vertex, Edge, VertexListParams, EdgeListParams } from '@/types/data';
import type { PaginatedResponse } from '@/types/api';

export const dataService = {
  vertices: {
    list: async (
      spaceName: string,
      tagName: string,
      params?: VertexListParams
    ): Promise<PaginatedResponse<Vertex>> => {
      const response = await get(`/api/spaces/${spaceName}/tags/${tagName}/vertices`)(params) as PaginatedResponse<Vertex>;
      return response;
    },
  },

  edges: {
    list: async (
      spaceName: string,
      edgeName: string,
      params?: EdgeListParams
    ): Promise<PaginatedResponse<Edge>> => {
      const response = await get(`/api/spaces/${spaceName}/edge-types/${edgeName}/edges`)(params) as PaginatedResponse<Edge>;
      return response;
    },
  },
};

export default dataService;
