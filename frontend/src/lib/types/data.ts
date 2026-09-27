// Domain types for graph visualization - kept as-is since they're frontend-specific
export interface Vertex {
  vid: string | number;
  tags: Record<string, Record<string, unknown>>;
}

export interface Edge {
  src: string | number;
  dst: string | number;
  edge_type: string;
  rank: number;
  properties: Record<string, unknown>;
}

// Query parameter types - can be aligned with generated schemas later if needed
export interface VertexListParams {
  limit?: number;
  offset?: number;
  filter?: string;
  sort_by?: string;
  sort_order?: 'ASC' | 'DESC';
}

export interface EdgeListParams {
  limit?: number;
  offset?: number;
  filter?: string;
  sort_by?: string;
  sort_order?: 'ASC' | 'DESC';
}
