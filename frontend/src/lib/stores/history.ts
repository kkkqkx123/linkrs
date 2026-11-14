import { writable } from 'svelte/store';
import { t } from '$i18n';
import { storage } from '$utils/storage';

export interface QueryHistoryItem {
	id: string;
	query: string;
	executionTime: number;
	timestamp: number;
	rowCount: number;
	success: boolean;
	path?: 'materialized' | 'stream';
	receivedCount?: number;
	reportedTotal?: number | null;
	streamStatus?: 'completed' | 'failed' | 'cancelled';
	errorCode?: string;
	traceId?: string;
}

export interface QueryFavoriteItem {
	id: string;
	name: string;
	query: string;
	createdAt: number;
	preferredPath?: 'materialized' | 'stream' | 'auto';
}

const MAX_HISTORY = 50;
const MAX_FAVORITES = 30;
// Own persistence key: the console store also writes its UI preferences, so
// sharing one key made the two writers overwrite each other's fields.
const STORAGE_KEY = 'console.history';

function generateId(): string {
	return `${Date.now()}-${Math.random().toString(36).substring(2, 9)}`;
}

function loadPersisted(): { history: QueryHistoryItem[]; favorites: QueryFavoriteItem[] } {
	const saved = storage.get<{ history?: unknown; favorites?: unknown }>(STORAGE_KEY);
	if (saved && Array.isArray(saved.history) && Array.isArray(saved.favorites)) {
		return {
			history: saved.history as QueryHistoryItem[],
			favorites: saved.favorites as QueryFavoriteItem[],
		};
	}
	return { history: [], favorites: [] };
}

function persist(history: QueryHistoryItem[], favorites: QueryFavoriteItem[]): void {
	storage.set(STORAGE_KEY, { history, favorites });
}

const persisted = loadPersisted();

export function createHistoryStore() {
	const { subscribe, update } = writable({
		history: persisted.history,
		favorites: persisted.favorites,
	});

	function addHistory(item: Omit<QueryHistoryItem, 'id' | 'timestamp'>): void {
		update((s) => {
			const newItem: QueryHistoryItem = {
				...item,
				id: generateId(),
				timestamp: Date.now(),
			};
			const newHistory = [newItem, ...s.history].slice(0, MAX_HISTORY);
			persist(newHistory, s.favorites);
			return { ...s, history: newHistory };
		});
	}

	function clearHistory(): void {
		update((s) => {
			persist([], s.favorites);
			return { ...s, history: [] };
		});
	}

	function addFavorite(name: string, query: string, preferredPath?: 'materialized' | 'stream' | 'auto'): { success: boolean; error?: string } {
		let result = { success: false, error: '' };
		update((s) => {
			if (!name.trim()) {
				result = { success: false, error: t('errors.favoriteNameRequired') };
				return s;
			}
			if (!query.trim()) {
				result = { success: false, error: t('errors.favoriteQueryRequired') };
				return s;
			}
			if (s.favorites.some((f) => f.name.toLowerCase() === name.toLowerCase())) {
				result = { success: false, error: t('errors.favoriteDuplicateName') };
				return s;
			}
			if (s.favorites.length >= MAX_FAVORITES) {
				result = { success: false, error: t('errors.favoriteLimitReached') };
				return s;
			}
			const newFav: QueryFavoriteItem = {
				id: generateId(),
				name: name.trim(),
				query: query.trim(),
				createdAt: Date.now(),
				preferredPath,
			};
			const newFavorites = [...s.favorites, newFav];
			persist(s.history, newFavorites);
			result = { success: true, error: '' };
			return { ...s, favorites: newFavorites };
		});
		return result;
	}

	function removeFavorite(id: string): void {
		update((s) => {
			const newFavorites = s.favorites.filter((f) => f.id !== id);
			persist(s.history, newFavorites);
			return { ...s, favorites: newFavorites };
		});
	}

	function isFavoriteNameExists(name: string): boolean {
		let exists = false;
		update((s) => {
			exists = s.favorites.some((f) => f.name.toLowerCase() === name.toLowerCase());
			return s;
		});
		return exists;
	}

	return {
		subscribe,
		addHistory,
		clearHistory,
		addFavorite,
		removeFavorite,
		isFavoriteNameExists,
	};
}

export const historyStore = createHistoryStore();
