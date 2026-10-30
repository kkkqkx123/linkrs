import { writable } from 'svelte/store';
import { storage, getLegacyRaw } from '$utils/storage';

export type Theme = 'light' | 'dark';

const STORAGE_KEY = 'appearance.theme';
// Key used before storage access was unified; read once as a fallback.
const LEGACY_STORAGE_KEY = 'linkrs-theme';

function initialTheme(): Theme {
	const saved =
		storage.get<Theme>(STORAGE_KEY) ?? getLegacyRaw(LEGACY_STORAGE_KEY);
	if (saved === 'dark' || saved === 'light') return saved;
	return window.matchMedia('(prefers-color-scheme: dark)').matches
		? 'dark'
		: 'light';
}

export const theme = writable<Theme>(initialTheme());

// Persistence lives here so every writer gets it for free; the `dark` class on
// the document is applied by the root layout.
theme.subscribe((value) => storage.set(STORAGE_KEY, value));

export function setTheme(next: Theme): void {
	theme.set(next);
}

export function toggleTheme(): void {
	theme.update((current) => (current === 'light' ? 'dark' : 'light'));
}
