// Auto-generated TypeScript types from OpenAPI schema
// DO NOT EDIT - Run npm run generate in frontend/codegen to update

export interface AddBatchItemsRequest {
  items: BatchItem[];
}

export interface AddBatchItemsResponse {
}

export interface AddFavoriteRequest {
}

export interface AddHistoryRequest {
}

export interface ApiError {
}

export interface ApiResponse_EdgeTypeDetail {
  data?: {
  indexes: IndexInfo[];
  properties: PropertyDef[];
};
  error?: null | ApiError;
}

export interface ApiResponse_FavoriteListResponse {
  data?: {
  items: FavoriteItem[];
};
  error?: null | ApiError;
}

export interface ApiResponse_HistoryListResponse {
  data?: {
  items: HistoryItem[];
};
  error?: null | ApiError;
}

export interface ApiResponse_IndexInfo {
  data?: {
  fields: unknown[];
};
  error?: null | ApiError;
}

export interface ApiResponse_PaginatedResponse_Value {
  data?: {
  items: unknown[];
};
  error?: null | ApiError;
}

export interface ApiResponse_SpaceDetail {
  data?: {
  statistics: SpaceStatistics;
};
  error?: null | ApiError;
}

export interface ApiResponse_SpaceStatistics {
  data?: {
};
  error?: null | ApiError;
}

export interface ApiResponse_TagDetail {
  data?: {
  indexes: IndexInfo[];
  properties: PropertyDef[];
};
  error?: null | ApiError;
}

export interface ApiResponse_Value {
  error?: null | ApiError;
}

export interface BatchErrorData {
  item_type: BatchItemType;
}

export type BatchItem = VertexData & unknown | EdgeData & unknown;

export type BatchItemType = string;

export interface BatchProgress {
}

export interface BatchQueryRequest {
  statements: unknown[];
}

export interface BatchQueryResponse {
  results: QueryResponse[];
}

export interface BatchResultData {
  errors?: BatchErrorData[];
}

export type BatchStatus = string;

export interface BatchStatusResponse {
  batch_id: String;
  progress: BatchProgress;
  status: BatchStatus;
}

export type BatchType = string;

export interface BeginTransactionRequest {
}

export interface ClearDegradedRequest {
}

export interface ClearFulltextIndexRequest {
}

export interface ClearFulltextIndexResponse {
}

export interface ClearVectorIndexRequest {
}

export interface ClearVectorIndexResponse {
}

export interface CreateBatchRequest {
  batch_type: BatchType;
}

export interface CreateBatchResponse {
  batch_id: String;
  status: BatchStatus;
}

export interface CreateEdgeTypeRequest {
  properties?: PropertyDef[];
}

export interface CreateIndexRequest {
  fields: unknown[];
}

export interface CreateSavepointRequest {
}

export interface CreateSessionRequest {
}

export interface CreateSpaceRequest {
}

export interface CreateTagRequest {
  properties?: PropertyDef[];
}

export interface CreateVectorIndexRequest {
  distance?: DistanceMetric;
}

export interface DeletePayloadRequest {
  keys: unknown[];
  point_ids: unknown[];
}

export type DistanceMetric = string;

export interface EdgeData {
}

export interface EdgeTypeDetail {
  indexes: IndexInfo[];
  properties: PropertyDef[];
}

export interface ExecuteBatchResponse {
  batch_id: String;
  result: BatchResultData;
  status: BatchStatus;
}

export interface FavoriteItem {
}

export interface FavoriteListResponse {
  items: FavoriteItem[];
}

export interface FulltextRebuildStatusResponse {
}

export interface HistoryItem {
}

export interface HistoryListResponse {
  items: HistoryItem[];
}

export interface ImportResponse {
}

export interface ImportStatusResponse {
}

export interface IndexInfo {
  fields: unknown[];
}

export interface ListVectorIndexesResponse {
  indexes: unknown[];
}

export interface LoginRequest {
}

export interface LoginResponse {
}

export interface LogoutRequest {
}

export interface MigrationExecuteRequest {
}

export interface MigrationRollbackRequest {
}

export interface PropertyDef {
}

export interface QueryData {
  columns?: unknown[];
  rows?: unknown[];
}

export interface QueryError {
}

export interface QueryMetadata {
}

export interface QueryRequest {
}

export interface QueryResponse {
  data?: null | QueryData;
  error?: null | QueryError;
  metadata?: QueryMetadata;
}

export interface RebuildFulltextIndexRequest {
}

export interface RebuildFulltextIndexResponse {
}

export interface RebuildVectorIndexRequest {
}

export interface RebuildVectorIndexResponse {
}

export interface RegisterFunctionRequest {
  parameters: unknown[];
}

export interface RequeueRequest {
}

export interface RetentionRunRequest {
}

export interface SavepointResponse {
}

export interface ScrollRequest {
}

export interface ScrollResponse {
  points: VectorSearchResult[];
}

export interface SessionResponse {
}

export interface SetPayloadRequest {
  point_ids: unknown[];
}

export interface SpaceDetail {
  statistics: SpaceStatistics;
}

export interface SpaceStatistics {
}

export interface StreamQueryRequest {
}

export type String = string;

export interface SyncStatusResponse {
}

export interface TagDetail {
  indexes: IndexInfo[];
  properties: PropertyDef[];
}

export interface TransactionResponse {
}

export interface UpdateConfigRequest {
}

export interface UpdateEdgeTypeRequest {
}

export interface UpdateFavoriteRequest {
}

export interface UpdateTagRequest {
}

export interface ValidateRequest {
  query: string;
  session_id: number;
  need_estimate?: boolean;
}

export interface ValidateResponse {
  valid: boolean;
  message: string;
  estimated_rows?: number | null;
}

export interface VectorFilter {
  min_should?: null | MinShouldCondition;
}

export interface VectorIndexDetailsResponse {
}

export interface VectorRebuildStatusResponse {
}

export interface VectorSearchRequest {
  filter?: null | VectorFilter;
  query_vector: unknown[];
}

export interface VectorSearchResponse {
  results: VectorSearchResult[];
}

export interface VectorSearchResult {
}

export interface VertexData {
}
