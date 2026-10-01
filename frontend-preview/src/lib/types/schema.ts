// Schema view types.
// The OpenAPI contract (`$lib/api/schema`, generated from `frontend/openapi.json`)
// is the single source of truth for wire shapes; this module only keeps
// frontend view types that have no contract equivalent.

import type { components } from '$lib/api/schema';

type PropertyDef = components['schemas']['PropertyDef'];

export interface Space {
  id: number;
  name: string;
  vid_type: string;
}

export interface Tag {
  id: number;
  name: string;
  properties: PropertyDef[];
  comment?: string;
  created_at: number;
}

export interface EdgeType {
  id: number;
  name: string;
  properties: PropertyDef[];
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

export type DataType =
  | 'STRING' | 'INT64' | 'DOUBLE' | 'BOOL'
  | 'DATETIME' | 'DATE' | 'TIME' | 'TIMESTAMP';

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
  byType: { tag: number; edge: number };
  byStatus: { creating: number; finished: number; failed: number; rebuilding: number };
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
