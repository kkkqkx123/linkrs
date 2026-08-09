import { writable } from 'svelte/store';
import type { VertexData, EdgeData, FilterGroup, Statistics, DataBrowserState } from '$types/dataBrowser';

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

function createDataBrowserStore() {
  const { subscribe, set, update } = writable<DataBrowserState>(initialState);

  return {
    subscribe,
    setActiveTab: (tab: 'vertices' | 'edges') => update(s => ({ ...s, activeTab: tab })),
    setSelectedTag: (tag: string | null) => update(s => ({ ...s, selectedTag: tag, vertexPage: 1, vertices: [], vertexTotal: 0 })),
    setSelectedEdgeType: (type: string | null) => update(s => ({ ...s, selectedEdgeType: type, edgePage: 1, edges: [], edgeTotal: 0 })),
    setVertices: (vertices: VertexData[], total: number) => update(s => ({ ...s, vertices, vertexTotal: total })),
    setEdges: (edges: EdgeData[], total: number) => update(s => ({ ...s, edges, edgeTotal: total })),
    setVertexPage: (page: number) => update(s => ({ ...s, vertexPage: page })),
    setEdgePage: (page: number) => update(s => ({ ...s, edgePage: page })),
    setVertexPageSize: (size: number) => update(s => ({ ...s, vertexPageSize: size, vertexPage: 1 })),
    setEdgePageSize: (size: number) => update(s => ({ ...s, edgePageSize: size, edgePage: 1 })),
    setVertexSort: (sort: { field: string; order: 'asc' | 'desc' } | null) => update(s => ({ ...s, vertexSort: sort })),
    setEdgeSort: (sort: { field: string; order: 'asc' | 'desc' } | null) => update(s => ({ ...s, edgeSort: sort })),
    setFilters: (filters: FilterGroup) => update(s => ({ ...s, filters })),
    addFilterCondition: (condition: FilterGroup['conditions'][0]) => update(s => ({
      ...s, filters: { ...s.filters, conditions: [...s.filters.conditions, condition] },
    })),
    removeFilterCondition: (index: number) => update(s => ({
      ...s, filters: { ...s.filters, conditions: s.filters.conditions.filter((_, i) => i !== index) },
    })),
    clearFilters: () => update(s => ({ ...s, filters: { conditions: [], logic: 'AND' }, vertexPage: 1, edgePage: 1 })),
    toggleFilterPanel: () => update(s => ({ ...s, filterPanelVisible: !s.filterPanelVisible })),
    setStatistics: (statistics: Statistics | null) => update(s => ({ ...s, statistics })),
    showDetail: (data: VertexData | EdgeData, type: 'vertex' | 'edge') => update(s => ({ ...s, detailData: data, detailType: type, detailModalVisible: true })),
    hideDetail: () => update(s => ({ ...s, detailModalVisible: false, detailData: null, detailType: null })),
    setLoading: (loading: boolean) => update(s => ({ ...s, loading })),
    setError: (error: string | null) => update(s => ({ ...s, error })),
    reset: () => set(initialState),
  };
}

export const dataBrowserStore = createDataBrowserStore();