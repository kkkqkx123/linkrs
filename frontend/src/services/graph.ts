import { get } from '@/utils/http';
import type { VertexDetail, EdgeDetail, Neighbor, NeighborParams } from '@/types/graph';

export const graphService = {
  vertices: {
    get: async (vid: string | number, space: string): Promise<VertexDetail> => {
      const response = await get(`/api/vertices/${vid}`)({ space }) as VertexDetail;
      return response;
    },

    getNeighbors: async (
      vid: string | number,
      space: string,
      params?: NeighborParams
    ): Promise<Neighbor[]> => {
      const response = await get(`/api/vertices/${vid}/neighbors`)({ space, ...params }) as Neighbor[];
      return response;
    },
  },

  edges: {
    get: async (
      src: string | number,
      dst: string | number,
      space: string,
      edgeType: string,
      rank?: number
    ): Promise<EdgeDetail> => {
      const response = await get('/api/edges')({
        space,
        src,
        dst,
        edge_type: edgeType,
        rank: rank ?? 0,
      }) as EdgeDetail;
      return response;
    },
  },
};

export default graphService;
