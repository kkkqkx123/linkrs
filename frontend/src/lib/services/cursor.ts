import { post, resolveSessionId } from '$utils/http';

export interface OpenCursorResult {
  cursorId: number;
  columns: string[];
}

export interface FetchCursorResult {
  columns: string[];
  rows: Record<string, unknown>[];
  hasMore: boolean;
  returned: number;
}

interface OpenEnvelope {
  cursor_id?: number;
  columns?: string[];
}

interface FetchEnvelope {
  columns?: string[];
  rows?: Record<string, unknown>[];
  has_more?: boolean;
  returned?: number;
}

export const cursorService = {
  /** Open a forward-only cursor over a single statement. */
  open: async (query: string, sessionId?: number): Promise<OpenCursorResult> => {
    const resolved = resolveSessionId(sessionId);
    if (resolved === undefined) {
      throw new Error('Missing session id for cursor open');
    }
    const response = await post<OpenEnvelope>('/v1/query/cursor/open', {
      session_id: resolved,
      query,
    });
    if (response.cursor_id === undefined) {
      throw new Error('Server returned no cursor id');
    }
    return { cursorId: response.cursor_id, columns: response.columns ?? [] };
  },

  /** Fetch one page; `hasMore` false means the execution is exhausted. */
  fetch: async (
    cursorId: number,
    pageSize: number,
    sessionId?: number,
  ): Promise<FetchCursorResult> => {
    const resolved = resolveSessionId(sessionId);
    if (resolved === undefined) {
      throw new Error('Missing session id for cursor fetch');
    }
    const response = await post<FetchEnvelope>('/v1/query/cursor/fetch', {
      session_id: resolved,
      cursor_id: cursorId,
      page_size: pageSize,
    });
    const rows = response.rows ?? [];
    return {
      columns: response.columns ?? [],
      rows,
      hasMore: response.has_more === true,
      returned: response.returned ?? rows.length,
    };
  },

  /** Release a cursor; unknown ids are ignored by the server. */
  close: async (cursorId: number, sessionId?: number): Promise<void> => {
    const resolved = resolveSessionId(sessionId);
    if (resolved === undefined) return;
    try {
      await post('/v1/query/cursor/close', {
        session_id: resolved,
        cursor_id: cursorId,
      });
    } catch {
      /* closing is best-effort; the server sweeps idle cursors */
    }
  },
};

export default cursorService;
