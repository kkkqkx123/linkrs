import { writable } from 'svelte/store';
import { schemaService } from '$services/schema';
import { queryService } from '$services/query';
import type { Space, SpaceDetail, SpaceStatistics, Tag, EdgeType, IndexInfo, CreateTagParams, CreateEdgeTypeParams, CreateIndexParams, UpdateTagParams, UpdateEdgeTypeParams } from '$types/schema';

export interface CreateSpaceParams {
  name: string;
  vidType: 'INT64' | 'FIXED_STRING(32)';
  partitionNum: number;
  replicaFactor: number;
}

interface SchemaState {
  spaces: Space[];
  isLoadingSpaces: boolean;
  spacesError: string | null;
  currentSpace: string | null;
  spaceDetails: Record<string, SpaceDetail>;
  spaceStatistics: Record<string, SpaceStatistics>;
  tags: Tag[];
  isLoadingTags: boolean;
  tagsError: string | null;
  edgeTypes: EdgeType[];
  isLoadingEdgeTypes: boolean;
  edgeTypesError: string | null;
  indexes: IndexInfo[];
  isLoadingIndexes: boolean;
  indexesError: string | null;
}

function persistCurrentSpace(name: string | null) {
  if (name) localStorage.setItem('schema-current-space', name);
  else localStorage.removeItem('schema-current-space');
}

function createSchemaStore() {
  const savedSpace = localStorage.getItem('schema-current-space');
  const { subscribe, set, update } = writable<SchemaState>({
    spaces: [], isLoadingSpaces: false, spacesError: null,
    currentSpace: savedSpace,
    spaceDetails: {}, spaceStatistics: {},
    tags: [], isLoadingTags: false, tagsError: null,
    edgeTypes: [], isLoadingEdgeTypes: false, edgeTypesError: null,
    indexes: [], isLoadingIndexes: false, indexesError: null,
  });

  return {
    subscribe,
    fetchSpaces: async () => {
      update(s => ({ ...s, isLoadingSpaces: true, spacesError: null }));
      try {
        const response = await schemaService.spaces.list();
        const spaces = Array.isArray(response) ? response : (response as { data?: Space[] }).data || [];
        update(s => {
          const newState = { ...s, spaces, isLoadingSpaces: false };
          if (!newState.currentSpace && spaces.length > 0) {
            newState.currentSpace = spaces[0].name;
            persistCurrentSpace(spaces[0].name);
          }
          return newState;
        });
      } catch (err: unknown) {
        update(s => ({ ...s, spacesError: err instanceof Error ? err.message : 'Failed to fetch spaces', isLoadingSpaces: false }));
      }
    },
    createSpace: async (params: CreateSpaceParams) => {
      const vidTypeStr = params.vidType === 'FIXED_STRING(32)' ? 'FIXED_STRING(32)' : 'INT64';
      const query = `CREATE SPACE IF NOT EXISTS ${params.name} (vid_type = ${vidTypeStr}, partition_num = ${params.partitionNum}, replica_factor = ${params.replicaFactor})`;
      await queryService.execute({ query });
      await this.fetchSpaces();
    },
    deleteSpace: async (name: string) => {
      const query = `DROP SPACE IF EXISTS ${name}`;
      await queryService.execute({ query });
      await this.fetchSpaces();
    },
    setCurrentSpace: (name: string | null) => {
      update(s => ({ ...s, currentSpace: name }));
      persistCurrentSpace(name);
    },
    fetchSpaceDetail: async (name: string) => {
      try {
        const detail = await schemaService.spaces.getDetail(name);
        update(s => ({ ...s, spaceDetails: { ...s.spaceDetails, [name]: detail } }));
      } catch (err) { console.error('Fetch space detail error:', err); }
    },
    fetchSpaceStatistics: async (name: string) => {
      try {
        const statistics = await schemaService.spaces.getStatistics(name);
        update(s => ({ ...s, spaceStatistics: { ...s.spaceStatistics, [name]: statistics } }));
      } catch (err) { console.error('Fetch space statistics error:', err); }
    },
    clearSpacesError: () => update(s => ({ ...s, spacesError: null })),
    fetchTags: async (spaceName: string) => {
      update(s => ({ ...s, isLoadingTags: true, tagsError: null }));
      try {
        const response = await schemaService.tags.list(spaceName);
        const tags = Array.isArray(response) ? response : (response as { data?: Tag[] }).data || [];
        update(s => ({ ...s, tags, isLoadingTags: false }));
      } catch (err: unknown) {
        update(s => ({ ...s, tagsError: err instanceof Error ? err.message : 'Failed to fetch tags', isLoadingTags: false }));
      }
    },
    createTag: async (spaceName: string, params: CreateTagParams) => {
      await schemaService.tags.create(spaceName, params);
      await this.fetchTags(spaceName);
    },
    updateTag: async (spaceName: string, tagName: string, params: UpdateTagParams) => {
      const queryParts: string[] = [];
      if (params.add_properties?.length) {
        const addProps = params.add_properties.map(p => `${p.name} ${p.data_type}${p.default_value ? ` DEFAULT ${p.default_value}` : ''}`).join(', ');
        queryParts.push(`ADD (${addProps})`);
      }
      if (params.drop_properties?.length) queryParts.push(`DROP (${params.drop_properties.join(', ')})`);
      if (queryParts.length > 0) {
        await queryService.execute({ query: `ALTER TAG ${tagName} ${queryParts.join(' ')}` });
        await this.fetchTags(spaceName);
      }
    },
    deleteTag: async (spaceName: string, tagName: string) => {
      await schemaService.tags.delete(spaceName, tagName);
      await this.fetchTags(spaceName);
    },
    clearTagsError: () => update(s => ({ ...s, tagsError: null })),
    fetchEdgeTypes: async (spaceName: string) => {
      update(s => ({ ...s, isLoadingEdgeTypes: true, edgeTypesError: null }));
      try {
        const response = await schemaService.edgeTypes.list(spaceName);
        const edgeTypes = Array.isArray(response) ? response : (response as { data?: EdgeType[] }).data || [];
        update(s => ({ ...s, edgeTypes, isLoadingEdgeTypes: false }));
      } catch (err: unknown) {
        update(s => ({ ...s, edgeTypesError: err instanceof Error ? err.message : 'Failed to fetch edge types', isLoadingEdgeTypes: false }));
      }
    },
    createEdgeType: async (spaceName: string, params: CreateEdgeTypeParams) => {
      await schemaService.edgeTypes.create(spaceName, params);
      await this.fetchEdgeTypes(spaceName);
    },
    updateEdgeType: async (spaceName: string, edgeName: string, params: UpdateEdgeTypeParams) => {
      const queryParts: string[] = [];
      if (params.add_properties?.length) {
        const addProps = params.add_properties.map(p => `${p.name} ${p.data_type}${p.default_value ? ` DEFAULT ${p.default_value}` : ''}`).join(', ');
        queryParts.push(`ADD (${addProps})`);
      }
      if (params.drop_properties?.length) queryParts.push(`DROP (${params.drop_properties.join(', ')})`);
      if (queryParts.length > 0) {
        await queryService.execute({ query: `ALTER EDGE ${edgeName} ${queryParts.join(' ')}` });
        await this.fetchEdgeTypes(spaceName);
      }
    },
    deleteEdgeType: async (spaceName: string, edgeName: string) => {
      await schemaService.edgeTypes.delete(spaceName, edgeName);
      await this.fetchEdgeTypes(spaceName);
    },
    clearEdgeTypesError: () => update(s => ({ ...s, edgeTypesError: null })),
    fetchIndexes: async (spaceName: string) => {
      update(s => ({ ...s, isLoadingIndexes: true, indexesError: null }));
      try {
        const response = await schemaService.indexes.list(spaceName);
        const indexes = Array.isArray(response) ? response : (response as { data?: IndexInfo[] }).data || [];
        update(s => ({ ...s, indexes, isLoadingIndexes: false }));
      } catch (err: unknown) {
        update(s => ({ ...s, indexesError: err instanceof Error ? err.message : 'Failed to fetch indexes', isLoadingIndexes: false }));
      }
    },
    createIndex: async (spaceName: string, params: CreateIndexParams) => {
      await schemaService.indexes.create(spaceName, params);
      await this.fetchIndexes(spaceName);
    },
    deleteIndex: async (spaceName: string, indexName: string) => {
      await schemaService.indexes.delete(spaceName, indexName);
      await this.fetchIndexes(spaceName);
    },
    rebuildIndex: async (spaceName: string, indexName: string) => {
      await schemaService.indexes.rebuild(spaceName, indexName);
      await this.fetchIndexes(spaceName);
    },
    clearIndexesError: () => update(s => ({ ...s, indexesError: null })),
  };
}

export const schemaStore = createSchemaStore();