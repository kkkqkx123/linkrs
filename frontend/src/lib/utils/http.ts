/**
 * Session helpers for API access.
 * JSON calls go through the typed client in `$lib/api/client`; this module
 * keeps the session storage helpers plus the base URL / header builders used
 * by non-JSON transports (SSE streaming in `streamQuery`, file downloads in
 * `export`), which intentionally bypass codegen. Not covered by codegen.
 */

export function getApiBaseUrl(): string {
	return import.meta.env.VITE_API_BASE_URL || 'http://localhost:9758';
}

export function getSessionHeaders(): Record<string, string> {
	const headers: Record<string, string> = { 'Content-Type': 'application/json' };
	try {
		const sessionId = localStorage.getItem('sessionId');
		if (sessionId) headers['X-Session-ID'] = sessionId;
	} catch {
		/* storage unavailable */
	}
	return headers;
}

/** Numeric session id for request bodies; undefined when not logged in. */
export function resolveSessionId(explicit?: number): number | undefined {
	if (explicit !== undefined) return explicit;
	try {
		const stored = localStorage.getItem('sessionId');
		if (!stored) return undefined;
		const parsed = Number(stored);
		return Number.isFinite(parsed) ? parsed : undefined;
	} catch {
		return undefined;
	}
}
