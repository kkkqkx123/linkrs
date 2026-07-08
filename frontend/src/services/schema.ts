import { get, post, _delete } from '@/utils/http';
import type {
  Space,
  SpaceDetail,
  Tag,
  TagDetail,
  EdgeType,
  EdgeTypeDetail,
  IndexInfo,
  CreateSpaceParams,
  CreateTagParams,
  CreateEdgeTypeParams,
  CreateIndexParams,
  DDLData,
} from '@/types/schema';

export const schemaService = {
  spaces: {
    list: async (): Promise<Space[]> => {
      const response = await get('/v1/schema/spaces')() as Space[];
      return response;
    },

    create: async (params: CreateSpaceParams): Promise<{ message: string; space_name: string }> => {
      const response = await post('/v1/schema/spaces')(params) as { message: string; space_name: string };
      return response;
    },

    get: async (name: string): Promise<{ space: Space }> => {
      const response = await get(`/v1/schema/spaces/${name}`)() as { space: Space };
      return response;
    },

    getDetail: async (name: string): Promise<SpaceDetail> => {
      const response = await get(`/v1/schema/spaces/${name}/details`)() as SpaceDetail;
      return response;
    },

    getStatistics: async (name: string): Promise<SpaceDetail['statistics']> => {
      const response = await get(`/v1/schema/spaces/${name}/statistics`)() as SpaceDetail['statistics'];
      return response;
    },

    delete: async (name: string): Promise<{ message: string; space_name: string }> => {
      const response = await _delete(`/v1/schema/spaces/${name}`)() as { message: string; space_name: string };
      return response;
    },
  },

  tags: {
    list: async (spaceName: string): Promise<Tag[]> => {
      const response = await get(`/v1/schema/spaces/${spaceName}/tags`)() as Tag[];
      return response;
    },

    create: async (spaceName: string, params: CreateTagParams): Promise<Tag> => {
      const response = await post(`/v1/schema/spaces/${spaceName}/tags`)(params) as Tag;
      return response;
    },

    getDetail: async (spaceName: string, tagName: string): Promise<TagDetail> => {
      const response = await get(`/v1/schema/spaces/${spaceName}/tags/${tagName}`)() as TagDetail;
      return response;
    },

    delete: async (spaceName: string, tagName: string): Promise<void> => {
      await _delete(`/v1/schema/spaces/${spaceName}/tags/${tagName}`)();
    },
  },

  edgeTypes: {
    list: async (spaceName: string): Promise<EdgeType[]> => {
      const response = await get(`/v1/schema/spaces/${spaceName}/edge-types`)() as EdgeType[];
      return response;
    },

    create: async (spaceName: string, params: CreateEdgeTypeParams): Promise<EdgeType> => {
      const response = await post(`/v1/schema/spaces/${spaceName}/edge-types`)(params) as EdgeType;
      return response;
    },

    getDetail: async (spaceName: string, edgeName: string): Promise<EdgeTypeDetail> => {
      const response = await get(`/v1/schema/spaces/${spaceName}/edge-types/${edgeName}`)() as EdgeTypeDetail;
      return response;
    },

    delete: async (spaceName: string, edgeName: string): Promise<void> => {
      await _delete(`/v1/schema/spaces/${spaceName}/edge-types/${edgeName}`)();
    },
  },

  indexes: {
    list: async (spaceName: string): Promise<IndexInfo[]> => {
      const response = await get(`/v1/schema/spaces/${spaceName}/indexes`)() as IndexInfo[];
      return response;
    },

    create: async (spaceName: string, params: CreateIndexParams): Promise<IndexInfo> => {
      const response = await post(`/v1/schema/spaces/${spaceName}/indexes`)(params) as IndexInfo;
      return response;
    },

    getDetail: async (spaceName: string, indexName: string): Promise<IndexInfo> => {
      const response = await get(`/v1/schema/spaces/${spaceName}/indexes/${indexName}`)() as IndexInfo;
      return response;
    },

    delete: async (spaceName: string, indexName: string): Promise<void> => {
      await _delete(`/v1/schema/spaces/${spaceName}/indexes/${indexName}`)();
    },

    rebuild: async (spaceName: string, indexName: string): Promise<void> => {
      await post(`/v1/schema/spaces/${spaceName}/indexes/${indexName}/rebuild`)();
    },
  },

  exportDDL: async (spaceName: string): Promise<DDLData> => {
    const response = await get(`/v1/schema/spaces/${spaceName}/ddl`)() as DDLData;
    return response;
  },
};

export default schemaService;
