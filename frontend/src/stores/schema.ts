import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import { schemaService } from '@/services/schema';
import { queryService } from '@/services/query';
import type {
  Space,
  SpaceDetail,
  SpaceStatistics,
  Tag,
  EdgeType,
  IndexInfo,
  CreateTagParams,
  CreateEdgeTypeParams,
  CreateIndexParams,
  UpdateTagParams,
  UpdateEdgeTypeParams,
} from '@/types/schema';

export interface CreateSpaceParams {
  name: string;
  vidType: 'INT64' | 'FIXED_STRING(32)';
  partitionNum: number;
  replicaFactor: number;
}

export interface SchemaState {
  // Space list
  spaces: Space[];
  isLoadingSpaces: boolean;
  spacesError: string | null;

  // Current space
  currentSpace: string | null;

  // Space details cache
  spaceDetails: Record<string, SpaceDetail>;
  spaceStatistics: Record<string, SpaceStatistics>;

  // Tags
  tags: Tag[];
  isLoadingTags: boolean;
  tagsError: string | null;

  // Edge types
  edgeTypes: EdgeType[];
  isLoadingEdgeTypes: boolean;
  edgeTypesError: string | null;

  // Indexes
  indexes: IndexInfo[];
  isLoadingIndexes: boolean;
  indexesError: string | null;

  // Actions - Space
  fetchSpaces: () => Promise<void>;
  createSpace: (params: CreateSpaceParams) => Promise<void>;
  deleteSpace: (name: string) => Promise<void>;
  setCurrentSpace: (name: string | null) => void;
  fetchSpaceDetail: (name: string) => Promise<void>;
  fetchSpaceStatistics: (name: string) => Promise<void>;
  clearSpacesError: () => void;

  // Actions - Tags
  fetchTags: (spaceName: string) => Promise<void>;
  createTag: (spaceName: string, params: CreateTagParams) => Promise<void>;
  updateTag: (spaceName: string, tagName: string, params: UpdateTagParams) => Promise<void>;
  deleteTag: (spaceName: string, tagName: string) => Promise<void>;
  clearTagsError: () => void;

  // Actions - Edge Types
  fetchEdgeTypes: (spaceName: string) => Promise<void>;
  createEdgeType: (spaceName: string, params: CreateEdgeTypeParams) => Promise<void>;
  updateEdgeType: (spaceName: string, edgeName: string, params: UpdateEdgeTypeParams) => Promise<void>;
  deleteEdgeType: (spaceName: string, edgeName: string) => Promise<void>;
  clearEdgeTypesError: () => void;

  // Actions - Indexes
  fetchIndexes: (spaceName: string) => Promise<void>;
  createIndex: (spaceName: string, params: CreateIndexParams) => Promise<void>;
  deleteIndex: (spaceName: string, indexName: string) => Promise<void>;
  rebuildIndex: (spaceName: string, indexName: string) => Promise<void>;
  clearIndexesError: () => void;
}

export const useSchemaStore = create<SchemaState>()(
  persist(
    (set, get) => ({
      // Initial state
      spaces: [],
      isLoadingSpaces: false,
      spacesError: null,
      currentSpace: null,
      spaceDetails: {},
      spaceStatistics: {},
      tags: [],
      isLoadingTags: false,
      tagsError: null,
      edgeTypes: [],
      isLoadingEdgeTypes: false,
      edgeTypesError: null,
      indexes: [],
      isLoadingIndexes: false,
      indexesError: null,

      // Space actions
      fetchSpaces: async () => {
        set({ isLoadingSpaces: true, spacesError: null });
        try {
          const response = await schemaService.spaces.list();
          // Handle case where API returns { data: [...] } or directly [...]
          const spaces = Array.isArray(response) ? response : (response as { data?: Space[] }).data || [];
          set({ spaces, isLoadingSpaces: false });

          const { currentSpace } = get();
          if (!currentSpace && spaces.length > 0) {
            set({ currentSpace: spaces[0].name });
          }
        } catch (err: unknown) {
          const errorMessage = err instanceof Error ? err.message : 'Failed to fetch spaces';
          set({ spacesError: errorMessage, isLoadingSpaces: false });
        }
      },

      createSpace: async (params: CreateSpaceParams) => {
        try {
          const vidTypeStr = params.vidType === 'FIXED_STRING(32)' ? 'FIXED_STRING(32)' : 'INT64';
          const query = `CREATE SPACE IF NOT EXISTS ${params.name} (vid_type = ${vidTypeStr}, partition_num = ${params.partitionNum}, replica_factor = ${params.replicaFactor})`;

          await queryService.execute({ query });
          await get().fetchSpaces();
        } catch (err: unknown) {
          const errorMessage = err instanceof Error ? err.message : 'Failed to create space';
          throw new Error(errorMessage);
        }
      },

      deleteSpace: async (name: string) => {
        try {
          const query = `DROP SPACE IF EXISTS ${name}`;
          await queryService.execute({ query });
          await get().fetchSpaces();

          const { currentSpace } = get();
          if (currentSpace === name) {
            const { spaces } = get();
            set({ currentSpace: spaces.length > 0 ? spaces[0].name : null });
          }
        } catch (err: unknown) {
          const errorMessage = err instanceof Error ? err.message : 'Failed to delete space';
          throw new Error(errorMessage);
        }
      },

      setCurrentSpace: (name: string | null) => {
        set({ currentSpace: name });
      },

      fetchSpaceDetail: async (name: string) => {
        try {
          const detail = await schemaService.spaces.getDetail(name);
          set((state) => ({
            spaceDetails: {
              ...state.spaceDetails,
              [name]: detail,
            },
          }));
        } catch (err: unknown) {
          const errorMessage = err instanceof Error ? err.message : 'Failed to fetch space detail';
          console.error('Fetch space detail error:', errorMessage);
        }
      },

      fetchSpaceStatistics: async (name: string) => {
        try {
          const statistics = await schemaService.spaces.getStatistics(name);
          set((state) => ({
            spaceStatistics: {
              ...state.spaceStatistics,
              [name]: statistics,
            },
          }));
        } catch (err: unknown) {
          const errorMessage = err instanceof Error ? err.message : 'Failed to fetch space statistics';
          console.error('Fetch space statistics error:', errorMessage);
        }
      },

      clearSpacesError: () => {
        set({ spacesError: null });
      },

      // Tag actions
      fetchTags: async (spaceName: string) => {
        set({ isLoadingTags: true, tagsError: null });
        try {
          const response = await schemaService.tags.list(spaceName);
          const tags = Array.isArray(response) ? response : (response as { data?: Tag[] }).data || [];
          set({ tags, isLoadingTags: false });
        } catch (err: unknown) {
          const errorMessage = err instanceof Error ? err.message : 'Failed to fetch tags';
          set({ tagsError: errorMessage, isLoadingTags: false });
        }
      },

      createTag: async (spaceName: string, params: CreateTagParams) => {
        try {
          await schemaService.tags.create(spaceName, params);
          await get().fetchTags(spaceName);
        } catch (err: unknown) {
          const errorMessage = err instanceof Error ? err.message : 'Failed to create tag';
          throw new Error(errorMessage);
        }
      },

      updateTag: async (spaceName: string, tagName: string, params: UpdateTagParams) => {
        try {
          const queryParts: string[] = [];

          if (params.add_properties && params.add_properties.length > 0) {
            const addProps = params.add_properties
              .map((p) => `${p.name} ${p.data_type}${p.default_value ? ` DEFAULT ${p.default_value}` : ''}`)
              .join(', ');
            queryParts.push(`ADD (${addProps})`);
          }

          if (params.drop_properties && params.drop_properties.length > 0) {
            const dropProps = params.drop_properties.join(', ');
            queryParts.push(`DROP (${dropProps})`);
          }

          if (queryParts.length > 0) {
            const query = `ALTER TAG ${tagName} ${queryParts.join(' ')}`;
            await queryService.execute({ query });
            await get().fetchTags(spaceName);
          }
        } catch (err: unknown) {
          const errorMessage = err instanceof Error ? err.message : 'Failed to update tag';
          throw new Error(errorMessage);
        }
      },

      deleteTag: async (spaceName: string, tagName: string) => {
        try {
          await schemaService.tags.delete(spaceName, tagName);
          await get().fetchTags(spaceName);
        } catch (err: unknown) {
          const errorMessage = err instanceof Error ? err.message : 'Failed to delete tag';
          throw new Error(errorMessage);
        }
      },

      clearTagsError: () => {
        set({ tagsError: null });
      },

      // Edge type actions
      fetchEdgeTypes: async (spaceName: string) => {
        set({ isLoadingEdgeTypes: true, edgeTypesError: null });
        try {
          const response = await schemaService.edgeTypes.list(spaceName);
          const edgeTypes = Array.isArray(response) ? response : (response as { data?: EdgeType[] }).data || [];
          set({ edgeTypes, isLoadingEdgeTypes: false });
        } catch (err: unknown) {
          const errorMessage = err instanceof Error ? err.message : 'Failed to fetch edge types';
          set({ edgeTypesError: errorMessage, isLoadingEdgeTypes: false });
        }
      },

      createEdgeType: async (spaceName: string, params: CreateEdgeTypeParams) => {
        try {
          await schemaService.edgeTypes.create(spaceName, params);
          await get().fetchEdgeTypes(spaceName);
        } catch (err: unknown) {
          const errorMessage = err instanceof Error ? err.message : 'Failed to create edge type';
          throw new Error(errorMessage);
        }
      },

      updateEdgeType: async (spaceName: string, edgeName: string, params: UpdateEdgeTypeParams) => {
        try {
          const queryParts: string[] = [];

          if (params.add_properties && params.add_properties.length > 0) {
            const addProps = params.add_properties
              .map((p) => `${p.name} ${p.data_type}${p.default_value ? ` DEFAULT ${p.default_value}` : ''}`)
              .join(', ');
            queryParts.push(`ADD (${addProps})`);
          }

          if (params.drop_properties && params.drop_properties.length > 0) {
            const dropProps = params.drop_properties.join(', ');
            queryParts.push(`DROP (${dropProps})`);
          }

          if (queryParts.length > 0) {
            const query = `ALTER EDGE ${edgeName} ${queryParts.join(' ')}`;
            await queryService.execute({ query });
            await get().fetchEdgeTypes(spaceName);
          }
        } catch (err: unknown) {
          const errorMessage = err instanceof Error ? err.message : 'Failed to update edge type';
          throw new Error(errorMessage);
        }
      },

      deleteEdgeType: async (spaceName: string, edgeName: string) => {
        try {
          await schemaService.edgeTypes.delete(spaceName, edgeName);
          await get().fetchEdgeTypes(spaceName);
        } catch (err: unknown) {
          const errorMessage = err instanceof Error ? err.message : 'Failed to delete edge type';
          throw new Error(errorMessage);
        }
      },

      clearEdgeTypesError: () => {
        set({ edgeTypesError: null });
      },

      // Index actions
      fetchIndexes: async (spaceName: string) => {
        set({ isLoadingIndexes: true, indexesError: null });
        try {
          const response = await schemaService.indexes.list(spaceName);
          const indexes = Array.isArray(response) ? response : (response as { data?: IndexInfo[] }).data || [];
          set({ indexes, isLoadingIndexes: false });
        } catch (err: unknown) {
          const errorMessage = err instanceof Error ? err.message : 'Failed to fetch indexes';
          set({ indexesError: errorMessage, isLoadingIndexes: false });
        }
      },

      createIndex: async (spaceName: string, params: CreateIndexParams) => {
        try {
          await schemaService.indexes.create(spaceName, params);
          await get().fetchIndexes(spaceName);
        } catch (err: unknown) {
          const errorMessage = err instanceof Error ? err.message : 'Failed to create index';
          throw new Error(errorMessage);
        }
      },

      deleteIndex: async (spaceName: string, indexName: string) => {
        try {
          await schemaService.indexes.delete(spaceName, indexName);
          await get().fetchIndexes(spaceName);
        } catch (err: unknown) {
          const errorMessage = err instanceof Error ? err.message : 'Failed to delete index';
          throw new Error(errorMessage);
        }
      },

      rebuildIndex: async (spaceName: string, indexName: string) => {
        try {
          await schemaService.indexes.rebuild(spaceName, indexName);
          await get().fetchIndexes(spaceName);
        } catch (err: unknown) {
          const errorMessage = err instanceof Error ? err.message : 'Failed to rebuild index';
          throw new Error(errorMessage);
        }
      },

      clearIndexesError: () => {
        set({ indexesError: null });
      },
    }),
    {
      name: 'schema-storage',
      partialize: (state) => ({ currentSpace: state.currentSpace }),
    }
  )
);
