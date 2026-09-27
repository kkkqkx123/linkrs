import { get } from '$utils/http';
import type { VertexDetail, WireEdgeDetail, Neighbor, NeighborParams } from '$types/graph';
import type { ApiResponse_Value } from '$types/schema';

export const graphService = {
  vertices: {
    get: async (vid: string | number, space: string): Promise<VertexDetail> =>
      await get(`/api/v1/graph/vertices/${vid}`, { space }) as any,
    getNeighbors: async (vid: string | number, space: string, params?: NeighborParams): Promise<Neighbor[]> =>
      await get<Array<Neighbor>>(
        `/api/v1/graph/vertices/${vid}/neighbors`,
        { space, ...params }
      ),
  },
  edges: {
    get: async (src: string | number, dst: string | number, space: string, edgeType: string, rank?: number): Promise<WireEdgeDetail> =>
      await get<WireEdgeDetail>(
        '/api/v1/graph/edges',
        { space, src, dst, edge_type: edgeType, rank: rank ?? 0 }
      ),
  },
};

export default graphService;
