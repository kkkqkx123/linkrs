import { writable, get } from 'svelte/store';
import { storage, getLegacyRaw } from '$utils/storage';

export type Density = 'comfortable' | 'dense';

const KEY = 'appearance.density';
// Key used before storage access was unified; read once as a fallback.
const LEGACY_KEY = 'linkrs.ui.density.v1';

interface StoredDensity {
	density: Density;
	userSet: boolean;
}

function load(): StoredDensity {
	const fallback = { density: 'comfortable' as Density, userSet: false };
	const parsed =
		storage.get<{ density?: unknown }>(KEY) ?? parseLegacyDensity();
	if (!parsed) return fallback;
	if (parsed.density === 'dense' || parsed.density === 'comfortable') {
		return { density: parsed.density, userSet: true };
	}
	return fallback;
}

function parseLegacyDensity(): { density?: unknown } | null {
	try {
		const raw = getLegacyRaw(LEGACY_KEY);
		if (!raw) return null;
		return JSON.parse(raw) as { density?: unknown };
	} catch {
		return null;
	}
}

const stored = load();

/** UI density: comfortable (default) or dense (compact tables/controls). */
export const density = writable<Density>(stored.density);

let userSet = stored.userSet;

density.subscribe((d) => {
	if (typeof document !== 'undefined') {
		document.documentElement.dataset.density = d;
	}
});

/** Persisted user choice. */
export function setDensity(d: Density): void {
	userSet = true;
	density.set(d);
	storage.set(KEY, { density: d });
}

export function toggleDensity(): void {
	setDensity(get(density) === 'dense' ? 'comfortable' : 'dense');
}

/**
 * Route default: ops-heavy pages start dense until the user picks a
 * density manually.
 */
export function applyRouteDensity(path: string): void {
	if (userSet) return;
	const defaultDense =
		path.startsWith('/monitoring') || path.startsWith('/sessions');
	density.set(defaultDense ? 'dense' : 'comfortable');
}
