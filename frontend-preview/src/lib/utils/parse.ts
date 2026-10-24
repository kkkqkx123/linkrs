export function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function asRecord(value: unknown): Record<string, unknown> {
	return isRecord(value) ? (value as Record<string, unknown>) : {};
}

export function asArray(value: unknown): Record<string, unknown>[] {
	return Array.isArray(value)
		? value.filter((item): item is Record<string, unknown> => isRecord(item))
		: [];
}

/** Pull a list out of the ad-hoc `{ key: [...] }` shapes of the bare endpoints. */
export function pickList(payload: unknown, keys: string[]): Record<string, unknown>[] {
	if (Array.isArray(payload)) return asArray(payload);
	if (!isRecord(payload)) return [];
	if (isRecord(payload.data)) {
		for (const key of keys) {
			if (Array.isArray(payload.data[key])) return asArray(payload.data[key]);
		}
		if (Array.isArray((payload.data as Record<string, unknown>).items))
			return asArray((payload.data as Record<string, unknown>).items);
	}
	for (const key of keys) {
		if (Array.isArray(payload[key])) return asArray(payload[key]);
	}
	return [];
}
