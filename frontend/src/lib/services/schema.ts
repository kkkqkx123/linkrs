import { get, post, _delete } from '$utils/http';
import type {
  Space, SpaceDetail, Tag, TagDetail, EdgeType, EdgeTypeDetail,
  IndexInfo, CreateSpaceParams, CreateTagParams, CreateEdgeTypeParams, CreateIndexParams, DDLData,
} from '$types/schema';

export const schemaService = {
  spaces: {
    list: async (): Promise<Space[]> => await get('/v1/schema/spaces')() as Space[],
    create: async (params: CreateSpaceParams): Promise<{ message: string; space_name: string }> =>
      await post('/v1/schema/spaces')(params) as { message: string; space_name: string },
    get: async (name: string): Promise<{ space: Space }> =>
      await get(`/v1/schema/spaces/${name}`)() as { space: Space },
    getDetail: async (name: string): Promise<SpaceDetail> =>
      await get(`/v1/schema/spaces/${name}/details`)() as SpaceDetail,
    getStatistics: async (name: string): Promise<SpaceDetail['statistics']> =>
      await get(`/v1/schema/spaces/${name}/statistics`)() as SpaceDetail['statistics'],
    delete: async (name: string): Promise<{ message: string; space_name: string }> =>
      await _delete(`/v1/schema/spaces/${name}`)() as { message: string; space_name: string },
  },
  tags: {
    list: async (spaceName: string): Promise<Tag[]> => await get(`/v1/schema/spaces/${spaceName}/tags`)() as Tag[],
    create: async (spaceName: string, params: CreateTagParams): Promise<Tag> =>
      await post(`/v1/schema/spaces/${spaceName}/tags`)(params) as Tag,
    getDetail: async (spaceName: string, tagName: string): Promise<TagDetail> =>
      await get(`/v1/schema/spaces/${spaceName}/tags/${tagName}`)() as TagDetail,
    delete: async (spaceName: string, tagName: string): Promise<void> => {
      await _delete(`/v1/schema/spaces/${spaceName}/tags/${tagName}`)();
    },
  },
  edgeTypes: {
    list: async (spaceName: string): Promise<EdgeType[]> => await get(`/v1/schema/spaces/${spaceName}/edge-types`)() as EdgeType[],
    create: async (spaceName: string, params: CreateEdgeTypeParams): Promise<EdgeType> =>
      await post(`/v1/schema/spaces/${spaceName}/edge-types`)(params) as EdgeType,
    getDetail: async (spaceName: string, edgeName: string): Promise<EdgeTypeDetail> =>
      await get(`/v1/schema/spaces/${spaceName}/edge-types/${edgeName}`)() as EdgeTypeDetail,
    delete: async (spaceName: string, edgeName: string): Promise<void> => {
      await _delete(`/v1/schema/spaces/${spaceName}/edge-types/${edgeName}`)();
    },
  },
  indexes: {
    list: async (spaceName: string): Promise<IndexInfo[]> => await get(`/v1/schema/spaces/${spaceName}/indexes`)() as IndexInfo[],
    create: async (spaceName: string, params: CreateIndexParams): Promise<IndexInfo> =>
      await post(`/v1/schema/spaces/${spaceName}/indexes`)(params) as IndexInfo,
    getDetail: async (spaceName: string, indexName: string): Promise<IndexInfo> =>
      await get(`/v1/schema/spaces/${spaceName}/indexes/${indexName}`)() as IndexInfo,
    delete: async (spaceName: string, indexName: string): Promise<void> => {
      await _delete(`/v1/schema/spaces/${spaceName}/indexes/${indexName}`)();
    },
    rebuild: async (spaceName: string, indexName: string): Promise<void> => {
      await post(`/v1/schema/spaces/${spaceName}/indexes/${indexName}/rebuild`)();
    },
  },
  exportDDL: async (spaceName: string): Promise<DDLData> =>
    await get(`/v1/schema/spaces/${spaceName}/ddl`)() as DDLData,
};

export default schemaService;