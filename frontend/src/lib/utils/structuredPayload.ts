export type PayloadShape = 'records' | 'strings' | 'primitive' | 'unknown';

export interface ClassifiedPayload {
	shape: PayloadShape;
	records: Record<string, unknown>[];
	values: string[];
}

/** Keys commonly wrapping a list inside an object envelope. */
const LIST_KEYS = ['indexes', 'items', 'results', 'points', 'data', 'list'] as const;

/**
 * Classify an opaque API payload for display. Arrays of objects render as
 * tables, arrays of scalars render as chip lists, and everything else falls
 * back to formatted JSON. Index-management endpoints document one shape but
 * return another depending on backend version, so the UI adapts at runtime
 * instead of binding to a single schema.
 */
export function classifyPayload(payload: unknown): ClassifiedPayload {
	if (Array.isArray(payload)) {
		if (payload.length === 0) {
			return { shape: 'strings', records: [], values: [] };
		}
		if (payload.every((item) => isPlainObject(item))) {
			return {
				shape: 'records',
				records: payload as Record<string, unknown>[],
				values: [],
			};
		}
		if (payload.every((item) => !isPlainObject(item))) {
			return {
				shape: 'strings',
				records: [],
				values: payload.map((item) => String(item)),
			};
		}
		return { shape: 'unknown', records: [], values: [] };
	}
	if (payload === null || payload === undefined) {
		return { shape: 'unknown', records: [], values: [] };
	}
	if (!isPlainObject(payload)) {
		return { shape: 'primitive', records: [], values: [String(payload)] };
	}
	// Unwrap an object envelope holding an obvious list field.
	const record = payload as Record<string, unknown>;
	for (const key of LIST_KEYS) {
		const inner = record[key];
		if (Array.isArray(inner)) return classifyPayload(inner);
	}
	return { shape: 'unknown', records: [], values: [] };
}

/** Union of all keys across the records, preserving first-seen order. */
export function recordColumns(records: Record<string, unknown>[]): string[] {
	const seen = new Set<string>();
	for (const row of records) {
		for (const key of Object.keys(row)) seen.add(key);
	}
	return [...seen];
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}
