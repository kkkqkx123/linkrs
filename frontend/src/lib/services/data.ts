import { call, client, unwrap } from '$lib/api/client';
import type { Vertex, Edge, VertexListParams, EdgeListParams } from '$types/data';
import type { components } from '$types/schema.gen';

type Paginated = components['schemas']['ApiResponse_PaginatedResponse_Value'];

export const dataService = {
	vertices: {
		list: async (spaceName: string, tagName: string, params?: VertexListParams): Promise<Vertex[]> => {
			const page = await unwrap(
				await call<Paginated>(
					client.GET('/api/v1/data/spaces/{name}/tags/{tag_name}/vertices', {
						params: {
							path: { name: spaceName, tag_name: tagName },
							query: {
								limit: params?.limit ?? 100,
								offset: params?.offset ?? 0,
								filter: params?.filter,
								sort_by: params?.sort_by,
								sort_order: params?.sort_order
							}
						}
					})
				)
			);
			return (page.items || []) as Vertex[];
		}
	},
	edges: {
		list: async (spaceName: string, edgeName: string, params?: EdgeListParams): Promise<Edge[]> => {
			const page = await unwrap(
				await call<Paginated>(
					client.GET('/api/v1/data/spaces/{name}/edge-types/{edge_name}/edges', {
						params: {
							path: { name: spaceName, edge_name: edgeName },
							query: {
								limit: params?.limit ?? 100,
								offset: params?.offset ?? 0,
								filter: params?.filter,
								sort_by: params?.sort_by,
								sort_order: params?.sort_order
							}
						}
					})
				)
			);
			return (page.items || []) as Edge[];
		}
	}
};

export default dataService;
