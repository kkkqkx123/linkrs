export function stringifyId(value: unknown): string | null {
	if (value === null || value === undefined) return null;
	if (typeof value === 'string') return value;
	if (
		typeof value === 'number' ||
		typeof value === 'bigint' ||
		typeof value === 'boolean'
	)
		return String(value);
	if (typeof value === 'object') {
		try {
			return JSON.stringify(value);
		} catch {
			return String(value);
		}
	}
	return String(value);
}

export function asRecord(value: unknown): Record<string, unknown> | null {
	if (typeof value === 'object' && value !== null && !Array.isArray(value)) {
		return value as Record<string, unknown>;
	}
	return null;
}
