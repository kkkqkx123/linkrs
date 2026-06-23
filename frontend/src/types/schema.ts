export interface Space {
  id: number;
  name: string;
  vid_type: string;
}

export interface SpaceDetail {
  id: number;
  name: string;
  vid_type: string;
  partition_num: number;
  replica_factor: number;
  comment?: string;
  created_at: number;
  statistics: SpaceStatistics;
}

export interface SpaceStatistics {
  vertex_count?: number;
  edge_count?: number;
}

export interface PropertyDef {
  name: string;
  data_type: string;
  nullable: boolean;
  default_value?: string;
  comment?: string;
}

export interface Tag {
  id: number;
  name: string;
  properties: PropertyDef[];
  comment?: string;
  created_at: number;
}

export interface TagDetail {
  id: number;
  name: string;
  properties: PropertyDef[];
  indexes: IndexInfo[];
  created_at: number;
}

export interface EdgeType {
  id: number;
  name: string;
  properties: PropertyDef[];
  comment?: string;
  created_at: number;
}

export interface EdgeTypeDetail {
  id: number;
  name: string;
  properties: PropertyDef[];
  indexes: IndexInfo[];
  created_at: number;
}

export interface IndexInfo {
  id: number;
  name: string;
  index_type: string;
  entity_type: string;
  entity_name: string;
  fields: string[];
  comment?: string;
  created_at: number;
}

export interface CreateSpaceParams {
  name: string;
  vid_type?: string;
  comment?: string;
}

export interface CreateTagParams {
  name: string;
  properties: PropertyDef[];
  ttlCol?: string;
  ttlDuration?: number;
}

export interface CreateEdgeTypeParams {
  name: string;
  properties: PropertyDef[];
  ttlCol?: string;
  ttlDuration?: number;
}

export interface CreateIndexParams {
  name: string;
  index_type: string;
  entity_type: string;
  entity_name: string;
  fields: string[];
  comment?: string;
}

// Extended types for Phase 4 & 5

export type DataType =
  | 'STRING'
  | 'INT64'
  | 'DOUBLE'
  | 'BOOL'
  | 'DATETIME'
  | 'DATE'
  | 'TIME'
  | 'TIMESTAMP';

export interface Property {
  name: string;
  type: DataType;
  default_value?: string;
  nullable?: boolean;
}

export type IndexStatus = 'creating' | 'finished' | 'failed' | 'rebuilding';

export interface Index {
  id: number;
  name: string;
  type: 'TAG' | 'EDGE';
  schemaName: string;
  properties: string[];
  status: IndexStatus;
  created_at: number;
  updated_at?: number;
  progress?: number;
  errorMessage?: string;
}

export interface IndexStats {
  total: number;
  byType: {
    tag: number;
    edge: number;
  };
  byStatus: {
    creating: number;
    finished: number;
    failed: number;
    rebuilding: number;
  };
}

export interface UpdateTagParams {
  add_properties?: PropertyDef[];
  drop_properties?: string[];
}

export interface UpdateEdgeTypeParams {
  add_properties?: PropertyDef[];
  drop_properties?: string[];
}

export interface DDLData {
  space: string;
  tags: string[];
  edges: string[];
  indexes: string[];
}