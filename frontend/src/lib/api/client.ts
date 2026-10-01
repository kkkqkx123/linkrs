/**
 * Typed API client built on openapi-fetch.
 * Paths, methods, params and bodies are checked against the generated
 * OpenAPI contract (schema.gen.d.ts).
 *
 * Response parsing preserves the previous json-bigint behavior (large
 * integers arrive as strings) via a custom fetch wrapper. The
 * `X-Session-ID` header is attached per request from localStorage.
 */

import createClient from 'openapi-fetch';
import JSONBigint from 'json-bigint';
import type { paths } from '$types/schema.gen';

export const BASE_URL = import.meta.env.VITE_API_BASE_URL || 'http://localhost:9758';

let onUnauthorized: (() => void) | null = null;

export function setUnauthorizedHandler(handler: () => void) {
	onUnauthorized = handler;
}

const JSONBig = JSONBigint({ storeAsString: true });

/** fetch wrapper that parses JSON with json-bigint (large ints stay strings). */
async function bigintFetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
	const response = await fetch(input, init);
	if (response.status === 401) {
		try {
			localStorage.removeItem('sessionId');
		} catch {
			/* storage unavailable */
		}
		onUnauthorized?.();
	}
	const contentType = response.headers.get('content-type') ?? '';
	if (!contentType.includes('application/json')) return response;
	const text = await response.text();
	let parsed: unknown;
	try {
		parsed = text ? JSONBig.parse(text) : null;
	} catch {
		parsed = text;
	}
	return new Response(JSON.stringify(parsed), {
		status: response.status,
		statusText: response.statusText,
		headers: response.headers
	});
}

export class ApiError extends Error {
	status: number;
	code?: string;

	constructor(message: string, status: number, code?: string) {
		super(message);
		this.name = 'ApiError';
		this.status = status;
		this.code = code;
	}
}

/** Backend failure payload shapes. */
interface FailureBody {
	error?: { code?: string; message?: string } | string | null;
	message?: string;
	status?: number;
}

function failureMessage(error: unknown, status: number): { message: string; code?: string } {
	if (error && typeof error === 'object') {
		const body = error as FailureBody;
		if (typeof body.error === 'string' && body.error) {
			return { message: body.error };
		}
		if (body.error && typeof body.error === 'object') {
			if (body.error.message) return { message: body.error.message, code: body.error.code };
		}
		if (typeof body.message === 'string' && body.message) return { message: body.message };
	}
	return { message: `HTTP ${status}` };
}

/**
 * Await an openapi-fetch call: throw ApiError on transport or backend
 * failure, otherwise return the response payload as-is.
 */
export async function call<T>(
	promise: Promise<{
		data?: unknown;
		error?: unknown;
		response?: Response;
	}>
): Promise<T> {
	const res = await promise;
	if (res.error !== undefined && res.error !== null) {
		const { message, code } = failureMessage(res.error, res.response?.status ?? 0);
		throw new ApiError(message, res.response?.status ?? 0, code);
	}
	return res.data as T;
}

/** Standard `{ success, data, error }` envelope shared by `/api/v1` web endpoints. */
export interface Envelope<T> {
	success: boolean;
	data?: T | null;
	error?: { code: string; message: string } | null;
}

/** Unwrap an envelope payload, throwing ApiError when `success` is false. */
export function unwrap<T>(envelope: Envelope<T>): T {
	if (!envelope.success || envelope.data === undefined || envelope.data === null) {
		throw new ApiError(envelope.error?.message ?? 'Request failed', 0, envelope.error?.code);
	}
	return envelope.data;
}

export const client = createClient<paths>({
	baseUrl: BASE_URL,
	headers: { 'Content-Type': 'application/json' },
	fetch: bigintFetch
});

/** Attach `X-Session-ID` on every request. */
client.use({
	async onRequest({ request }) {
		try {
			const sessionId = localStorage.getItem('sessionId');
			if (sessionId) request.headers.set('X-Session-ID', sessionId);
		} catch {
			/* storage unavailable */
		}
		return request;
	}
});
