// Re-export generated types from schema.d.ts
export type { ApiResponse_Value as ApiResponse, ApiError } from '$types/schema';

export interface PaginatedResponse<T> {
  items: T[];
  total: number;
  limit: number;
  offset: number;
}

// For backward compatibility with services expecting specific response shape
export type DataPaginatedResponse<T> = {
  items: T[];
  total: number;
};
