export interface VertexDetail {
  vid: string | number;
  tags: Record<string, Record<string, unknown>>;
}

export interface EdgeDetail {
  src: string | number;
  dst: string | number;
  edge_type: string;
  rank: number;
  properties: Record<string, unknown>;
}

export interface NeighborParams {
  direction?: 'OUT' | 'IN' | 'BOTH';
  edge_type?: string;
}

export interface Neighbor {
  vid: string | number;
  edge_type: string;
  direction: 'OUT' | 'IN';
  rank: number;
}

// Graph visualization types
export interface GraphNode {
  id: string;
  tag: string;
  properties: Record<string, unknown>;
}

export interface GraphEdge {
  id: string;
  type: string;
  source: string;
  target: string;
  rank: number;
  properties: Record<string, unknown>;
}

export interface GraphData {
  nodes: GraphNode[];
  edges: GraphEdge[];
}

export type LayoutType = 'force' | 'circle' | 'grid' | 'hierarchical';

export interface GraphStyleConfig {
  nodes: Record<string, {
    color: string;
    size: 'small' | 'medium' | 'large';
    labelProperty: string;
  }>;
  edges: Record<string, {
    color: string;
    width: 'thin' | 'medium' | 'thick';
    labelProperty: string;
  }>;
}

export interface NodeDetail {
  id: string;
  tag: string;
  properties: Record<string, unknown>;
}

export interface EdgeDetailInfo {
  id: string;
  type: string;
  source: string;
  target: string;
  rank: number;
  properties: Record<string, unknown>;
}
