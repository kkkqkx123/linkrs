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