import JSONBigint from 'json-bigint';

const JSONBig = JSONBigint({ storeAsString: true });

export interface SchemaEvent {
  kind: 'schema';
  columns: string[];
  stmt: number;
}

export interface RowEvent {
  kind: 'row';
  row: Record<string, unknown>;
  index: number;
  stmt: number;
}

export interface MetadataEvent {
  kind: 'metadata';
  rowsReturned: number;
  executionTimeMs: number;
  stmt: number;
}

export interface StreamErrorEvent {
  kind: 'error';
  code: string;
  message: string;
  stmt: number;
}

export interface DoneEvent {
  kind: 'done';
}

export interface StatementBeginEvent {
  kind: 'statement_begin';
  index: number;
  query: string;
}

export interface StreamStatementEnd {
  kind: 'statement_end';
  index: number;
  success: boolean;
  rowsReturned: number;
  executionTimeMs: number;
  code: string | null;
  message: string | null;
}

export type StreamEvent =
  | SchemaEvent
  | RowEvent
  | MetadataEvent
  | StreamErrorEvent
  | DoneEvent
  | StatementBeginEvent
  | StreamStatementEnd;

export function parseJsonTolerant(text: string): unknown {
  const trimmed = text.trim();
  if (!trimmed) return null;
  try {
    return JSONBig.parse(trimmed);
  } catch {
    return JSON.parse(trimmed);
  }
}

interface RawFrame {
  event: string;
  dataLines: string[];
}

function splitRawFrames(buffer: string): { frames: string[]; rest: string } {
  const frames: string[] = [];
  let start = 0;
  let i = 0;
  while (i < buffer.length) {
    const ch = buffer[i];
    if (ch === '\n' || ch === '\r') {
      const lineEnd = i;
      let next = i + 1;
      if (ch === '\r' && buffer[next] === '\n') next += 1;
      let after = next;
      if (buffer[after] === '\n') {
        after += 1;
      } else if (buffer[after] === '\r') {
        after += 1;
        if (buffer[after] === '\n') after += 1;
      } else {
        i = next;
        continue;
      }
      frames.push(buffer.slice(start, lineEnd));
      start = after;
      i = after;
      continue;
    }
    i += 1;
  }
  return { frames, rest: buffer.slice(start) };
}

function parseFrame(raw: string): RawFrame {
  const out: RawFrame = { event: '', dataLines: [] };
  const lines = raw.split(/\r\n|\r|\n/);
  for (const line of lines) {
    if (!line) continue;
    if (line.startsWith(':')) continue;
    const colon = line.indexOf(':');
    if (colon === -1) continue;
    const field = line.slice(0, colon);
    let value = line.slice(colon + 1);
    if (value.startsWith(' ')) value = value.slice(1);
    if (field === 'event') out.event = value;
    else if (field === 'data') out.dataLines.push(value);
  }
  return out;
}

function asStringArray(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.filter((item): item is string => typeof item === 'string');
}

function toPlainValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(toPlainValue);
  if (value !== null && typeof value === 'object') {
    const out: Record<string, unknown> = {};
    for (const [key, entry] of Object.entries(value as Record<string, unknown>)) {
      out[key] = toPlainValue(entry);
    }
    return out;
  }
  return value;
}

function readStmt(record: Record<string, unknown>): number {
  const stmt = record['stmt'];
  if (typeof stmt === 'number' && Number.isInteger(stmt) && stmt >= 0) return stmt;
  return 0;
}

function recordFor(payload: unknown): Record<string, unknown> {
  if (payload !== null && typeof payload === 'object' && !Array.isArray(payload)) {
    return payload as Record<string, unknown>;
  }
  return {};
}

function frameToEvent(frame: RawFrame): StreamEvent | null {
  const name = frame.event || 'message';
  if (name === 'done') return { kind: 'done' };
  if (frame.dataLines.length === 0) return null;
  const payload = toPlainValue(parseJsonTolerant(frame.dataLines.join('\n'))) as Record<
    string,
    unknown
  > | null;
  if (name === 'statement_begin') {
    const record = (payload ?? {}) as Record<string, unknown>;
    const index = typeof record['index'] === 'number' ? record['index'] : -1;
    if (index < 0) return null;
    const query = typeof record['query'] === 'string' ? record['query'] : '';
    return { kind: 'statement_begin', index, query };
  }
  if (name === 'statement_end') {
    const record = (payload ?? {}) as Record<string, unknown>;
    const index = typeof record['index'] === 'number' ? record['index'] : -1;
    if (index < 0) return null;
    const rowsReturned = typeof record['rows_returned'] === 'number' ? record['rows_returned'] : 0;
    const executionTimeMs =
      typeof record['execution_time_ms'] === 'number' ? record['execution_time_ms'] : 0;
    return {
      kind: 'statement_end',
      index,
      success: record['success'] === true,
      rowsReturned,
      executionTimeMs,
      code: typeof record['code'] === 'string' ? record['code'] : null,
      message: typeof record['message'] === 'string' ? record['message'] : null,
    };
  }
  if (name === 'schema') {
    const columns = asStringArray((payload as Record<string, unknown> | null)?.['columns']);
    return { kind: 'schema', columns, stmt: readStmt(recordFor(payload)) };
  }
  if (name === 'metadata') {
    const record = (payload ?? {}) as Record<string, unknown>;
    const rowsReturned = typeof record['rows_returned'] === 'number' ? record['rows_returned'] : 0;
    const executionTimeMs =
      typeof record['execution_time_ms'] === 'number' ? record['execution_time_ms'] : 0;
    return { kind: 'metadata', rowsReturned, executionTimeMs, stmt: readStmt(record) };
  }
  if (name === 'error') {
    const record = (payload ?? {}) as Record<string, unknown>;
    const message = typeof record['message'] === 'string' ? record['message'] : 'Query failed';
    const code = typeof record['code'] === 'string' ? record['code'] : 'QUERY_ERROR';
    return { kind: 'error', code, message, stmt: readStmt(record) };
  }
  const record = (payload ?? {}) as Record<string, unknown>;
  const row =
    record['row'] && typeof record['row'] === 'object'
      ? (record['row'] as Record<string, unknown>)
      : {};
  const index = typeof record['index'] === 'number' ? record['index'] : -1;
  if (index < 0) return null;
  return { kind: 'row', row, index, stmt: readStmt(record) };
}

export class SseParser {
  private buffer = '';
  private doneSeen = false;

  feed(chunk: string): StreamEvent[] {
    if (this.doneSeen) return [];
    this.buffer += chunk;
    const { frames, rest } = splitRawFrames(this.buffer);
    this.buffer = rest;
    const events: StreamEvent[] = [];
    for (const raw of frames) {
      const frame = parseFrame(raw);
      const event = frameToEvent(frame);
      if (!event) continue;
      events.push(event);
      if (event.kind === 'done') {
        this.doneSeen = true;
        this.buffer = '';
        break;
      }
    }
    return events;
  }

  get doneReceived(): boolean {
    return this.doneSeen;
  }

  get pendingBytes(): number {
    return this.buffer.length;
  }
}
