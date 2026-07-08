import { create } from 'zustand';
import type {
  VertexData,
  EdgeData,
  FilterGroup,
  Statistics,
  DataBrowserState,
} from '@/types/dataBrowser';

export interface DataBrowserStore extends DataBrowserState {
  // Actions
  setActiveTab: (tab: 'vertices' | 'edges') => void;
  setSelectedTag: (tag: string | null) => void;
  setSelectedEdgeType: (type: string | null) => void;
  setVertices: (vertices: VertexData[], total: number) => void;
  setEdges: (edges: EdgeData[], total: number) => void;
  setVertexPage: (page: number) => void;
  setEdgePage: (page: number) => void;
  setVertexPageSize: (size: number) => void;
  setEdgePageSize: (size: number) => void;
  setVertexSort: (sort: { field: string; order: 'asc' | 'desc' } | null) => void;
  setEdgeSort: (sort: { field: string; order: 'asc' | 'desc' } | null) => void;
  setFilters: (filters: FilterGroup) => void;
  addFilterCondition: (condition: FilterGroup['conditions'][0]) => void;
  removeFilterCondition: (index: number) => void;
  clearFilters: () => void;
  toggleFilterPanel: () => void;
  setStatistics: (stats: Statistics | null) => void;
  showDetail: (data: VertexData | EdgeData, type: 'vertex' | 'edge') => void;
  hideDetail: () => void;
  setLoading: (loading: boolean) => void;
  setError: (error: string | null) => void;
  reset: () => void;
}

const initialState: DataBrowserState = {
  activeTab: 'vertices',
  selectedTag: null,
  selectedEdgeType: null,
  vertices: [],
  edges: [],
  vertexTotal: 0,
  edgeTotal: 0,
  vertexPage: 1,
  edgePage: 1,
  vertexPageSize: 50,
  edgePageSize: 50,
  vertexSort: null,
  edgeSort: null,
  filters: { conditions: [], logic: 'AND' },
  filterPanelVisible: false,
  statistics: null,
  detailModalVisible: false,
  detailData: null,
  detailType: null,
  loading: false,
  error: null,
};

export const useDataBrowserStore = create<DataBrowserStore>((set, get) => ({
  ...initialState,

  setActiveTab: (tab) => set({ activeTab: tab }),

  setSelectedTag: (tag) =>
    set({
      selectedTag: tag,
      vertexPage: 1,
      vertices: [],
      vertexTotal: 0,
    }),

  setSelectedEdgeType: (type) =>
    set({
      selectedEdgeType: type,
      edgePage: 1,
      edges: [],
      edgeTotal: 0,
    }),

  setVertices: (vertices, total) => set({ vertices, vertexTotal: total }),

  setEdges: (edges, total) => set({ edges, edgeTotal: total }),

  setVertexPage: (page) => set({ vertexPage: page }),

  setEdgePage: (page) => set({ edgePage: page }),

  setVertexPageSize: (size) => set({ vertexPageSize: size, vertexPage: 1 }),

  setEdgePageSize: (size) => set({ edgePageSize: size, edgePage: 1 }),

  setVertexSort: (sort) => set({ vertexSort: sort }),

  setEdgeSort: (sort) => set({ edgeSort: sort }),

  setFilters: (filters) => set({ filters }),

  addFilterCondition: (condition) => {
    const { filters } = get();
    set({
      filters: {
        ...filters,
        conditions: [...filters.conditions, condition],
      },
    });
  },

  removeFilterCondition: (index) => {
    const { filters } = get();
    set({
      filters: {
        ...filters,
        conditions: filters.conditions.filter((_, i) => i !== index),
      },
    });
  },

  clearFilters: () =>
    set({
      filters: { conditions: [], logic: 'AND' },
      vertexPage: 1,
      edgePage: 1,
    }),

  toggleFilterPanel: () =>
    set((state) => ({ filterPanelVisible: !state.filterPanelVisible })),

  setStatistics: (statistics) => set({ statistics }),

  showDetail: (data, type) =>
    set({ detailData: data, detailType: type, detailModalVisible: true }),

  hideDetail: () =>
    set({ detailModalVisible: false, detailData: null, detailType: null }),

  setLoading: (loading) => set({ loading }),

  setError: (error) => set({ error }),

  reset: () => set(initialState),
}));
