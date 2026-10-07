/**
 * Typed model for the migration SSE stream
 * (`GET /v1/migration/stream/{space}/{label}`).
 *
 * Event names and payload shapes mirror the server's `migration_event_to_sse`
 * mapping; unknown or malformed frames are skipped, never thrown.
 */

export type MigrationProgressEvent =
	| {
			kind: 'started';
			planHash: string;
			space: string;
			label: string;
			isEdge: boolean;
	  }
	| { kind: 'step_started'; stepIdx: number }
	| { kind: 'step_completed'; stepIdx: number; rows: number }
	| {
			kind: 'completed';
			success: boolean;
			stepsCompleted: number;
			rowsMigrated: number;
			errors: string[];
	  }
	| { kind: 'failed'; error: string }
	| {
			kind: 'rolled_back';
			success: boolean;
			stepsCompleted: number;
			rowsMigrated: number;
	  };

const TERMINAL = new Set(['completed', 'failed', 'rolled_back']);

export function isTerminalMigrationEvent(
	event: MigrationProgressEvent,
): boolean {
	return TERMINAL.has(event.kind);
}

function asRecord(payload: unknown): Record<string, unknown> {
	if (payload !== null && typeof payload === 'object' && !Array.isArray(payload)) {
		return payload as Record<string, unknown>;
	}
	return {};
}

function asNonNegativeInt(value: unknown, fallback = 0): number {
	return typeof value === 'number' &&
		Number.isInteger(value) &&
		value >= 0
		? value
		: fallback;
}

function asString(value: unknown, fallback = ''): string {
	return typeof value === 'string' ? value : fallback;
}

function asStringArray(value: unknown): string[] {
	if (!Array.isArray(value)) return [];
	return value.filter(
		(item): item is string => typeof item === 'string',
	);
}

/**
 * Parse one SSE frame into a typed migration event.
 * Returns `null` for unknown names or malformed payloads.
 */
export function parseMigrationEvent(
	name: string,
	payload: unknown,
): MigrationProgressEvent | null {
	const record = asRecord(payload);
	switch (name) {
		case 'started':
			return {
				kind: 'started',
				planHash: asString(record['plan_hash']),
				space: asString(record['space']),
				label: asString(record['label']),
				isEdge: record['is_edge'] === true,
			};
		case 'step_started': {
			const stepIdx = record['step_idx'];
			if (typeof stepIdx !== 'number' || !Number.isInteger(stepIdx) || stepIdx < 0)
				return null;
			return { kind: 'step_started', stepIdx };
		}
		case 'step_completed': {
			const stepIdx = record['step_idx'];
			if (typeof stepIdx !== 'number' || !Number.isInteger(stepIdx) || stepIdx < 0)
				return null;
			return {
				kind: 'step_completed',
				stepIdx,
				rows: asNonNegativeInt(record['rows']),
			};
		}
		case 'completed':
			return {
				kind: 'completed',
				success: record['success'] === true,
				stepsCompleted: asNonNegativeInt(record['steps_completed']),
				rowsMigrated: asNonNegativeInt(record['rows_migrated']),
				errors: asStringArray(record['errors']),
			};
		case 'failed':
			return { kind: 'failed', error: asString(record['error']) };
		case 'rolled_back':
			return {
				kind: 'rolled_back',
				success: record['success'] === true,
				stepsCompleted: asNonNegativeInt(record['steps_completed']),
				rowsMigrated: asNonNegativeInt(record['rows_migrated']),
			};
		default:
			return null;
	}
}

interface RawFrame {
	event: string;
	data: string[];
}

/**
 * Incremental SSE frame splitter for the migration stream.
 * Comment keepalives (`: ...`) are discarded; terminal lifecycle events
 * (`completed`/`failed`/`rolled_back`) mark the stream finished.
 */
export class MigrationSseParser {
	private buffer = '';
	private finishedSeen = false;

	feed(chunk: string): MigrationProgressEvent[] {
		if (this.finishedSeen) return [];
		this.buffer += chunk;
		const events: MigrationProgressEvent[] = [];
		for (;;) {
			const match = /\r\n\r\n|\n\n|\r\r/.exec(this.buffer);
			if (!match) break;
			const raw = this.buffer.slice(0, match.index);
			this.buffer = this.buffer.slice(match.index + match[0].length);
			const frame = parseRawFrame(raw);
			if (!frame) continue;
			const payload = parsePayload(frame.data.join('\n'));
			const event = parseMigrationEvent(frame.event, payload);
			if (!event) continue;
			events.push(event);
			if (isTerminalMigrationEvent(event)) {
				this.finishedSeen = true;
				this.buffer = '';
				break;
			}
		}
		return events;
	}

	get finished(): boolean {
		return this.finishedSeen;
	}
}

function parseRawFrame(raw: string): RawFrame | null {
	let event = '';
	const data: string[] = [];
	for (const line of raw.split(/\r\n|\r|\n/)) {
		if (!line || line.startsWith(':')) continue;
		const colon = line.indexOf(':');
		if (colon === -1) continue;
		const field = line.slice(0, colon);
		const value = line.slice(colon + 1).startsWith(' ')
			? line.slice(colon + 2)
			: line.slice(colon + 1);
		if (field === 'event') event = value;
		else if (field === 'data') data.push(value);
	}
	if (!event || data.length === 0) return null;
	return { event, data };
}

function parsePayload(text: string): unknown {
	const trimmed = text.trim();
	if (!trimmed) return null;
	try {
		return JSON.parse(trimmed);
	} catch {
		return null;
	}
}
