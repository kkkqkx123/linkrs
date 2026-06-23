export interface VertexData {
  id: string;
  tag: string;
  properties: Record<string, unknown>;
}

export interface EdgeData {
  id: string;
  type: string;
  src: string;
  dst: string;
  rank: number;
  properties: Record<string, unknown>;
}

export type FilterOperator =
  | 'eq'
  | 'ne'
  | 'gt'
  | 'lt'
  | 'ge'
  | 'le'
  | 'contains'
  | 'startsWith'
  | 'endsWith';

export interface FilterCondition {
  property: string;
  operator: FilterOperator;
  value: string | number | boolean;
}

export interface FilterGroup {
  conditions: FilterCondition[];
  logic: 'AND' | 'OR';
}

export interface Statistics {
  totalVertices: number;
  totalEdges: number;
  tagCount: number;
  edgeTypeCount: number;
  tagDistribution: { tag: string; count: number }[];
  edgeTypeDistribution: { type: string; count: number }[];
}

export interface VertexListResponse {
  data: VertexData[];
  total: number;
  page: number;
  pageSize: number;
}

export interface EdgeListResponse {
  data: EdgeData[];
  total: number;
  page: number;
  pageSize: number;
}

export interface DataBrowserState {
  activeTab: 'vertices' | 'edges';
  selectedTag: string | null;
  selectedEdgeType: string | null;
  vertices: VertexData[];
  edges: EdgeData[];
  vertexTotal: number;
  edgeTotal: number;
  vertexPage: number;
  edgePage: number;
  vertexPageSize: number;
  edgePageSize: number;
  vertexSort: { field: string; order: 'asc' | 'desc' } | null;
  edgeSort: { field: string; order: 'asc' | 'desc' } | null;
  filters: FilterGroup;
  filterPanelVisible: boolean;
  statistics: Statistics | null;
  detailModalVisible: boolean;
  detailData: VertexData | EdgeData | null;
  detailType: 'vertex' | 'edge' | null;
  loading: boolean;
  error: string | null;
}
