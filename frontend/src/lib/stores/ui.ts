import { writable, get } from 'svelte/store';

export type Density = 'comfortable' | 'dense';

const KEY = 'linkrs.ui.density.v1';

interface StoredDensity {
	density: Density;
	userSet: boolean;
}

function load(): StoredDensity {
	try {
		const raw = localStorage.getItem(KEY);
		if (raw) {
			const parsed = JSON.parse(raw) as { density?: unknown };
			if (parsed.density === 'dense' || parsed.density === 'comfortable') {
				return { density: parsed.density, userSet: true };
			}
		}
	} catch {
		/* storage unavailable */
	}
	return { density: 'comfortable', userSet: false };
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
	try {
		localStorage.setItem(KEY, JSON.stringify({ density: d }));
	} catch {
		/* storage unavailable */
	}
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
