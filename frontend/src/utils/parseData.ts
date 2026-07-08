import type { QueryResult, QueryError } from '@/types/query';

// Parse query result from API response
export const parseQueryResult = (response: unknown): QueryResult | null => {
  if (!response || typeof response !== 'object') {
    return null;
  }

  const data = response as Record<string, unknown>;

  // Handle different response formats
  if (data.columns && Array.isArray(data.columns) && data.rows && Array.isArray(data.rows)) {
    return {
      columns: data.columns as string[],
      rows: data.rows as unknown[][],
      rowCount: (data.rowCount as number) || (data.rows as unknown[]).length,
      data: [],
    };
  }

  // Handle nested data structure
  if (data.data && typeof data.data === 'object') {
    return parseQueryResult(data.data);
  }

  return {
    columns: [],
    rows: [],
    rowCount: 0,
    data: [],
  };
};

// Parse error from API response
export const parseQueryError = (response: unknown): QueryError | null => {
  if (!response || typeof response !== 'object') {
    return null;
  }

  const data = response as Record<string, unknown>;

  if (data.error && typeof data.error === 'object') {
    const error = data.error as Record<string, unknown>;
    return {
      code: (error.code as string) || 'UNKNOWN_ERROR',
      message: (error.message as string) || 'Unknown error occurred',
      position: error.position as { line: number; column: number } | undefined,
    };
  }

  if (data.message && typeof data.message === 'string') {
    return {
      code: (data.code as string) || 'ERROR',
      message: data.message,
    };
  }

  return null;
};

// Format cell value for display
export const formatCellValue = (value: unknown): string => {
  if (value === null || value === undefined) {
    return 'null';
  }

  if (typeof value === 'boolean') {
    return value ? 'true' : 'false';
  }

  if (typeof value === 'number') {
    return value.toString();
  }

  if (typeof value === 'string') {
    return value;
  }

  if (typeof value === 'object') {
    if (Array.isArray(value)) {
      return `[${value.length} items]`;
    }
    return JSON.stringify(value);
  }

  return String(value);
};

// Get column width based on content
export const getColumnWidth = (column: string, rows: unknown[][], minWidth = 80, maxWidth = 300): number => {
  const headerWidth = column.length * 10 + 32; // 10px per char + padding
  
  let maxContentWidth = headerWidth;
  
  for (const row of rows.slice(0, 50)) { // Check first 50 rows
    const value = row[0]; // This is simplified, should get correct column index
    const formatted = formatCellValue(value);
    const contentWidth = Math.min(formatted.length * 8 + 32, maxWidth);
    maxContentWidth = Math.max(maxContentWidth, contentWidth);
  }

  return Math.max(minWidth, Math.min(maxContentWidth, maxWidth));
};

// Truncate text with ellipsis
export const truncateText = (text: string, maxLength: number): string => {
  if (!text || text.length <= maxLength) {
    return text;
  }
  return text.substring(0, maxLength - 3) + '...';
};

// Format execution time
export const formatExecutionTime = (ms: number): string => {
  if (ms < 1) {
    return '< 1 ms';
  }
  if (ms < 1000) {
    return `${Math.round(ms)} ms`;
  }
  return `${(ms / 1000).toFixed(2)} s`;
};

// Format row count
export const formatRowCount = (count: number): string => {
  if (count === 0) {
    return '0 rows';
  }
  if (count === 1) {
    return '1 row';
  }
  if (count < 1000) {
    return `${count} rows`;
  }
  if (count < 1000000) {
    return `${(count / 1000).toFixed(1)}K rows`;
  }
  return `${(count / 1000000).toFixed(1)}M rows`;
};

// Convert result to array of objects (for easier processing)
export const resultToObjects = (result: QueryResult): Record<string, unknown>[] => {
  if (!result || !result.columns || !result.rows) {
    return [];
  }

  return result.rows.map((row) => {
    const obj: Record<string, unknown> = {};
    result.columns.forEach((col, index) => {
      obj[col] = row[index];
    });
    return obj;
  });
};

// Check if value is numeric
export const isNumeric = (value: unknown): boolean => {
  if (typeof value === 'number') {
    return !isNaN(value);
  }
  if (typeof value === 'string') {
    return !isNaN(Number(value)) && value.trim() !== '';
  }
  return false;
};

// Sort result rows
export const sortRows = (
  rows: unknown[][],
  columnIndex: number,
  direction: 'asc' | 'desc'
): unknown[][] => {
  return [...rows].sort((a, b) => {
    const aVal = a[columnIndex];
    const bVal = b[columnIndex];

    // Handle null values
    if (aVal === null && bVal === null) return 0;
    if (aVal === null) return direction === 'asc' ? -1 : 1;
    if (bVal === null) return direction === 'asc' ? 1 : -1;

    // Numeric comparison
    if (isNumeric(aVal) && isNumeric(bVal)) {
      const aNum = Number(aVal);
      const bNum = Number(bVal);
      return direction === 'asc' ? aNum - bNum : bNum - aNum;
    }

    // String comparison
    const aStr = String(aVal).toLowerCase();
    const bStr = String(bVal).toLowerCase();
    
    if (aStr < bStr) return direction === 'asc' ? -1 : 1;
    if (aStr > bStr) return direction === 'asc' ? 1 : -1;
    return 0;
  });
};
