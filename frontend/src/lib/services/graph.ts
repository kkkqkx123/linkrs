import { get } from '$utils/http';
import type { VertexDetail, EdgeDetail, Neighbor, NeighborParams } from '$types/graph';

export const graphService = {
  vertices: {
    get: async (vid: string | number, space: string): Promise<VertexDetail> =>
      await get(`/api/vertices/${vid}`)({ space }) as VertexDetail,
    getNeighbors: async (vid: string | number, space: string, params?: NeighborParams): Promise<Neighbor[]> =>
      await get(`/api/vertices/${vid}/neighbors`)({ space, ...params }) as Neighbor[],
  },
  edges: {
    get: async (src: string | number, dst: string | number, space: string, edgeType: string, rank?: number): Promise<EdgeDetail> =>
      await get('/api/edges')({ space, src, dst, edge_type: edgeType, rank: rank ?? 0 }) as EdgeDetail,
  },
};

export default graphService;