import { BASE_URL } from '$lib/api/client';

/**
 * Session helpers for API access.
 * JSON calls go through the typed client in `$lib/api/client`; this module
 * keeps the session storage helpers plus the base URL / header builders used
 * by non-JSON transports (SSE streaming in `streamQuery`, file downloads in
 * `export`), which intentionally bypass codegen. Not covered by codegen.
 */

import { STORAGE_KEYS } from '$config/constants';
import { storage, getLegacyRaw } from '$utils/storage';

export function getApiBaseUrl(): string {
	return BASE_URL;
}

/** Session id persistence via the unified storage helpers; the pre-unification
 * raw entry is kept as a one-time read fallback. */
function getStoredSessionId(): string | null {
	return storage.get<string>(STORAGE_KEYS.SESSION_ID) ?? getLegacyRaw(STORAGE_KEYS.SESSION_ID);
}

function setStoredSessionId(id: string): void {
	storage.set(STORAGE_KEYS.SESSION_ID, id);
}

function clearStoredSessionId(): void {
	storage.remove(STORAGE_KEYS.SESSION_ID);
	try {
		// Drop the pre-unification raw entry as well.
		localStorage.removeItem(STORAGE_KEYS.SESSION_ID);
	} catch {
		/* storage unavailable */
	}
}

export function getSessionHeaders(): Record<string, string> {
	const headers: Record<string, string> = {
		'Content-Type': 'application/json',
	};
	const sessionId = getStoredSessionId();
	if (sessionId) headers['X-Session-ID'] = sessionId;
	return headers;
}

/** Numeric session id for request bodies; undefined when not logged in. */
export function resolveSessionId(explicit?: number): number | undefined {
	if (explicit !== undefined) return explicit;
	const stored = getStoredSessionId();
	if (!stored) return undefined;
	const parsed = Number(stored);
	return Number.isFinite(parsed) ? parsed : undefined;
}

/** Persist the session id after login; shared with stream/connection. */
export function storeSessionId(sessionId: string | number): void {
	setStoredSessionId(String(sessionId));
}

/** Drop the stored session id (logout, expired, unauthorized). */
export function clearSessionId(): void {
	clearStoredSessionId();
}
