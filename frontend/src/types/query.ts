export interface HistoryItem {
  id: string;
  session_id: string;
  query: string;
  executed_at: string;
  execution_time_ms: number;
  rows_returned: number;
  success: boolean;
  error_message?: string;
}

export interface HistoryParams {
  query: string;
  execution_time_ms: number;
  rows_returned: number;
  success: boolean;
  error_message?: string;
}

export interface FavoriteItem {
  id: string;
  session_id: string;
  name: string;
  query: string;
  description?: string;
  created_at: string;
}

export interface FavoriteParams {
  name: string;
  query: string;
  description?: string;
}

export interface UpdateFavoriteParams {
  name?: string;
  query?: string;
  description?: string;
}

// Query execution types
export interface QueryResult {
  columns: string[];
  rows: unknown[][];
  rowCount: number;
  data: unknown[];
}

export interface QueryError {
  code: string;
  message: string;
  position?: {
    line: number;
    column: number;
  };
}