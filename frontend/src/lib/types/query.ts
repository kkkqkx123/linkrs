export interface QueryResult {
  columns: string[];
  rows: Record<string, unknown>[];
  rowCount: number;
}

export interface QueryError {
  code: string;
  message: string;
  position?: {
    line: number;
    column: number;
  };
}