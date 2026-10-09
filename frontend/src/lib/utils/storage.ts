const STORAGE_PREFIX = 'linkrs_';

export function estimateSize(value: unknown): number {
	try {
		return JSON.stringify(value).length * 2;
	} catch {
		return 0;
	}
}

export function getStorageUsage(): number {
	let total = 0;
	try {
		for (let i = 0; i < localStorage.length; i++) {
			const key = localStorage.key(i);
			if (key && key.startsWith(STORAGE_PREFIX)) {
				const item = localStorage.getItem(key);
				if (item) total += item.length * 2;
			}
		}
	} catch {
		/* ignore */
	}
	return total;
}

export function isStorageNearCapacity(threshold = 0.8): boolean {
	try {
		const usage = getStorageUsage();
		const limit = 5 * 1024 * 1024;
		return usage > limit * threshold;
	} catch {
		return false;
	}
}

export function safeSetItem(key: string, value: unknown): boolean {
	try {
		const serialized = JSON.stringify(value);
		localStorage.setItem(`${STORAGE_PREFIX}${key}`, serialized);
		return true;
	} catch (error) {
		if (error instanceof DOMException && error.name === 'QuotaExceededError') {
			console.warn('localStorage quota exceeded, evicting old data...');
			evictOldestEntries();
			try {
				localStorage.setItem(`${STORAGE_PREFIX}${key}`, JSON.stringify(value));
				return true;
			} catch {
				console.error('Failed to save after eviction');
				return false;
			}
		}
		console.error('Error saving to localStorage:', error);
		return false;
	}
}

function evictOldestEntries(): void {
	try {
		const entries: { key: string; timestamp: number }[] = [];
		for (let i = 0; i < localStorage.length; i++) {
			const key = localStorage.key(i);
			if (key && key.startsWith(STORAGE_PREFIX)) {
				const item = localStorage.getItem(key);
				if (item) {
					try {
						const parsed = JSON.parse(item);
						const timestamp = parsed.timestamp || parsed.createdAt || 0;
						entries.push({ key, timestamp });
					} catch {
						entries.push({ key, timestamp: 0 });
					}
				}
			}
		}
		entries.sort((a, b) => a.timestamp - b.timestamp);
		const toRemove = Math.ceil(entries.length * 0.2);
		for (let i = 0; i < toRemove && i < entries.length; i++) {
			localStorage.removeItem(entries[i].key);
		}
	} catch {
		/* ignore */
	}
}

export const storage = {
	set: (key: string, value: unknown): void => {
		safeSetItem(key, value);
	},

	get: <T>(key: string, defaultValue?: T): T | null => {
		try {
			const item = localStorage.getItem(`${STORAGE_PREFIX}${key}`);
			if (item === null) return defaultValue ?? null;
			return JSON.parse(item) as T;
		} catch (error) {
			console.error('Error reading from localStorage:', error);
			return defaultValue ?? null;
		}
	},

	remove: (key: string): void => {
		try {
			localStorage.removeItem(`${STORAGE_PREFIX}${key}`);
		} catch (error) {
			console.error('Error removing from localStorage:', error);
		}
	},

	clear: (): void => {
		try {
			const keys = Object.keys(localStorage);
			keys.forEach((key) => {
				if (key.startsWith(STORAGE_PREFIX)) localStorage.removeItem(key);
			});
		} catch (error) {
			console.error('Error clearing localStorage:', error);
		}
	},

	has: (key: string): boolean => {
		try {
			return localStorage.getItem(`${STORAGE_PREFIX}${key}`) !== null;
		} catch {
			return false;
		}
	},
};

export const session = {
	set: (key: string, value: unknown): void => {
		try {
			sessionStorage.setItem(`${STORAGE_PREFIX}${key}`, JSON.stringify(value));
		} catch (error) {
			console.error('Error saving to sessionStorage:', error);
		}
	},

	get: <T>(key: string, defaultValue?: T): T | null => {
		try {
			const item = sessionStorage.getItem(`${STORAGE_PREFIX}${key}`);
			if (item === null) return defaultValue ?? null;
			return JSON.parse(item) as T;
		} catch (error) {
			console.error('Error reading from sessionStorage:', error);
			return defaultValue ?? null;
		}
	},

	remove: (key: string): void => {
		try {
			sessionStorage.removeItem(`${STORAGE_PREFIX}${key}`);
		} catch (error) {
			console.error('Error removing from sessionStorage:', error);
		}
	},

	clear: (): void => {
		try {
			const keys = Object.keys(sessionStorage);
			keys.forEach((key) => {
				if (key.startsWith(STORAGE_PREFIX)) sessionStorage.removeItem(key);
			});
		} catch (error) {
			console.error('Error clearing sessionStorage:', error);
		}
	},
};