import { get, post, _delete } from '$utils/http';
import type {
  SpaceDetail, TagDetail, EdgeTypeDetail, IndexInfo,
  CreateSpaceRequest, CreateTagRequest, CreateEdgeTypeRequest, CreateIndexRequest,
} from '$types/schema';
import type { ApiResponse_SpaceDetail, ApiResponse_TagDetail, ApiResponse_EdgeTypeDetail, ApiResponse_IndexInfo, ApiResponse_SpaceStatistics } from '$types/schema';

export const schemaService = {
  spaces: {
    list: async (): Promise<SpaceDetail[]> => 
      await get<{ data?: SpaceDetail[] }>('/api/v1/schema/spaces').then(res => res.data || []),
    create: async (params: CreateSpaceRequest): Promise<{ message: string; space_name: string }> =>
      await post<{ message: string; space_name: string }>('/api/v1/schema/spaces', params),
    get: async (name: string): Promise<{ space: SpaceDetail }> =>
      await get<{ space: SpaceDetail }>(`/api/v1/schema/spaces/${name}`),
    getDetail: async (name: string): Promise<SpaceDetail> =>
      await get<ApiResponse_SpaceDetail>(`/api/v1/schema/spaces/${name}/details`)
        .then(res => res.data!),
    getStatistics: async (name: string): Promise<SpaceDetail['statistics']> =>
      await get<ApiResponse_SpaceStatistics>(`/api/v1/schema/spaces/${name}/statistics`)
        .then(res => res.data!),
    delete: async (name: string): Promise<{ message: string; space_name: string }> =>
      await _delete<{ message: string; space_name: string }>(`/api/v1/schema/spaces/${name}`),
  },
  tags: {
    list: async (spaceName: string): Promise<TagDetail[]> =>
      await get<{ data?: TagDetail[] }>(`/api/v1/schema/spaces/${spaceName}/tags`).then(res => res.data || []),
    create: async (spaceName: string, params: CreateTagRequest): Promise<TagDetail> =>
      await post<TagDetail>(`/api/v1/schema/spaces/${spaceName}/tags`, params),
    getDetail: async (spaceName: string, tagName: string): Promise<TagDetail> =>
      await get<ApiResponse_TagDetail>(`/api/v1/schema/spaces/${spaceName}/tags/${tagName}`)
        .then(res => res.data!),
    delete: async (spaceName: string, tagName: string): Promise<void> => {
      await _delete<void>(`/api/v1/schema/spaces/${spaceName}/tags/${tagName}`);
    },
  },
  edgeTypes: {
    list: async (spaceName: string): Promise<EdgeTypeDetail[]> =>
      await get<{ data?: EdgeTypeDetail[] }>(`/api/v1/schema/spaces/${spaceName}/edge-types`).then(res => res.data || []),
    create: async (spaceName: string, params: CreateEdgeTypeRequest): Promise<EdgeTypeDetail> =>
      await post<EdgeTypeDetail>(`/api/v1/schema/spaces/${spaceName}/edge-types`, params),
    getDetail: async (spaceName: string, edgeName: string): Promise<EdgeTypeDetail> =>
      await get<ApiResponse_EdgeTypeDetail>(`/api/v1/schema/spaces/${spaceName}/edge-types/${edgeName}`)
        .then(res => res.data!),
    delete: async (spaceName: string, edgeName: string): Promise<void> => {
      await _delete<void>(`/api/v1/schema/spaces/${spaceName}/edge-types/${edgeName}`);
    },
  },
  indexes: {
    list: async (spaceName: string): Promise<IndexInfo[]> =>
      await get<{ data?: IndexInfo[] }>(`/api/v1/schema/spaces/${spaceName}/indexes`)
        .then(res => res.data || []),
    create: async (spaceName: string, params: CreateIndexRequest): Promise<IndexInfo> =>
      await post<IndexInfo>(`/api/v1/schema/spaces/${spaceName}/indexes`, params),
    getDetail: async (spaceName: string, indexName: string): Promise<IndexInfo> =>
      await get<IndexInfo>(`/api/v1/schema/spaces/${spaceName}/indexes/${indexName}`),
    delete: async (spaceName: string, indexName: string): Promise<void> => {
      await _delete<void>(`/api/v1/schema/spaces/${spaceName}/indexes/${indexName}`);
    },
    rebuild: async (spaceName: string, indexName: string): Promise<void> => {
      await post<void>(`/api/v1/schema/spaces/${spaceName}/indexes/${indexName}/rebuild`);
    },
  },
};

export default schemaService;
