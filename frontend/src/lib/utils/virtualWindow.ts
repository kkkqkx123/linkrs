export interface WindowRange {
  start: number;
  end: number;
  topSpacer: number;
  bottomSpacer: number;
}

export function computeWindow(
  total: number,
  scrollTop: number,
  viewportHeight: number,
  rowHeight: number,
  overscanScreens = 1,
): WindowRange {
  if (total <= 0 || viewportHeight <= 0 || rowHeight <= 0) {
    return { start: 0, end: 0, topSpacer: 0, bottomSpacer: 0 };
  }
  const safeTop = Math.max(0, scrollTop);
  const visibleStart = Math.floor(safeTop / rowHeight);
  const visibleCount = Math.max(1, Math.ceil(viewportHeight / rowHeight));
  const overscan = Math.max(0, overscanScreens) * visibleCount;
  const start = Math.max(0, Math.min(total, visibleStart - overscan));
  const visibleEnd = Math.min(total, visibleStart + visibleCount);
  const end = Math.max(visibleEnd, Math.min(total, visibleEnd + overscan));
  return {
    start,
    end,
    topSpacer: start * rowHeight,
    bottomSpacer: (total - end) * rowHeight,
  };
}

export const STREAM_JSON_PREVIEW_LIMIT = 200;
export const STREAM_COLUMN_SAMPLE_LIMIT = 50;
export const STREAM_MIN_COLUMN_WIDTH = 96;
export const STREAM_MAX_COLUMN_WIDTH = 340;

export function estimateColumnWidths(
  columns: string[],
  sampleRows: Record<string, unknown>[],
  formatCell: (value: unknown) => string,
): number[] {
  return columns.map((column) => {
    let longest = column.length;
    const limit = Math.min(sampleRows.length, STREAM_COLUMN_SAMPLE_LIMIT);
    for (let i = 0; i < limit; i += 1) {
      const text = formatCell(sampleRows[i][column]);
      if (text.length > longest) longest = text.length;
    }
    const px = Math.ceil(longest * 7.5 + 24);
    return Math.max(STREAM_MIN_COLUMN_WIDTH, Math.min(STREAM_MAX_COLUMN_WIDTH, px));
  });
}
