export interface QueryResult {
  columns: string[];
  rows: Record<string, unknown>[];
  rowCount: number;
  /** The server cut the result at the configured row ceiling. */
  truncated?: boolean;
}

export interface QueryError {
  code: string;
  message: string;
  position?: {
    line: number;
    column: number;
  };
}