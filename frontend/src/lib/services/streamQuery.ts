import { SseParser } from '$utils/sseParser';
import { getApiBaseUrl, getSessionHeaders, resolveSessionId } from '$utils/http';

export interface StreamRowHandler {
  onSchema?: (columns: string[], stmt: number) => void;
  onRow?: (row: Record<string, unknown>, index: number, stmt: number) => void;
  onMetadata?: (info: { rowsReturned: number; executionTimeMs: number; stmt: number }) => void;
  onStreamError?: (error: { code: string; message: string; stmt: number }) => void;
  onStatementBegin?: (info: { index: number; query: string }) => void;
  onStatementEnd?: (info: {
    index: number;
    success: boolean;
    rowsReturned: number;
    executionTimeMs: number;
    code: string | null;
    message: string | null;
  }) => void;
}

export interface StreamQueryOptions {
  query: string;
  sessionId?: number;
  signal?: AbortSignal;
  /** Batch-streaming statements. When present, `parameters` and
   * `sessionVariables` ride along, mirroring the materialized batch call. */
  statements?: string[];
  parameters?: Record<string, unknown>;
  sessionVariables?: Record<string, unknown>;
  failFast?: boolean;
}

export interface StreamQueryOutcome {
  cancelled: boolean;
  doneReceived: boolean;
  rowsReturned: number;
  executionTimeMs: number;
  streamError: { code: string; message: string } | null;
}

export function isAbortError(error: unknown): boolean {
  return error instanceof DOMException && error.name === 'AbortError';
}

export async function streamQuery(
  options: StreamQueryOptions,
  handlers: StreamRowHandler,
): Promise<StreamQueryOutcome> {
  const sessionId = resolveSessionId(options.sessionId);
  if (sessionId === undefined) {
    throw new Error('Missing session id for stream execution');
  }
  const parser = new SseParser();
  const outcome: StreamQueryOutcome = {
    cancelled: false,
    doneReceived: false,
    rowsReturned: 0,
    executionTimeMs: 0,
    streamError: null,
  };
  let response: Response;
  const batchMode = (options.statements?.length ?? 0) > 0;
  const body: Record<string, unknown> = batchMode
    ? {
        query: '',
        session_id: sessionId,
        statements: options.statements,
        parameters: options.parameters ?? {},
        session_variables: options.sessionVariables ?? {},
        fail_fast: options.failFast ?? true,
      }
    : { query: options.query, session_id: sessionId };
  try {
    response = await fetch(`${getApiBaseUrl()}/v1/query/stream`, {
      method: 'POST',
      headers: { ...getSessionHeaders(), Accept: 'text/event-stream' },
      body: JSON.stringify(body),
      signal: options.signal,
    });
  } catch (error) {
    if (isAbortError(error) || options.signal?.aborted) {
      outcome.cancelled = true;
      return outcome;
    }
    throw error;
  }
  if (!response.ok || !response.body) {
    const detail = await response.text().catch(() => '');
    throw new Error(`Stream request failed with status ${response.status}${detail ? `: ${detail.slice(0, 200)}` : ''}`);
  }
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      const events = parser.feed(decoder.decode(value, { stream: true }));
      for (const event of events) {
        if (event.kind === 'schema') handlers.onSchema?.(event.columns, event.stmt);
        else if (event.kind === 'row') handlers.onRow?.(event.row, event.index, event.stmt);
        else if (event.kind === 'metadata') {
          outcome.rowsReturned = event.rowsReturned;
          outcome.executionTimeMs = event.executionTimeMs;
          handlers.onMetadata?.({ rowsReturned: event.rowsReturned, executionTimeMs: event.executionTimeMs, stmt: event.stmt });
        } else if (event.kind === 'error') {
          outcome.streamError = { code: event.code, message: event.message };
          handlers.onStreamError?.({ code: event.code, message: event.message, stmt: event.stmt });
        } else if (event.kind === 'statement_begin') {
          handlers.onStatementBegin?.({ index: event.index, query: event.query });
        } else if (event.kind === 'statement_end') {
          handlers.onStatementEnd?.({
            index: event.index,
            success: event.success,
            rowsReturned: event.rowsReturned,
            executionTimeMs: event.executionTimeMs,
            code: event.code,
            message: event.message,
          });
        }
      }
      if (parser.doneReceived) break;
    }
    parser.feed(decoder.decode());
  } catch (error) {
    if (isAbortError(error) || options.signal?.aborted) {
      outcome.cancelled = true;
      return outcome;
    }
    throw error;
  } finally {
    reader.releaseLock();
  }
  outcome.doneReceived = parser.doneReceived;
  return outcome;
}
