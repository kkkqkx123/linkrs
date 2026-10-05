/**
 * Typed API client built on openapi-fetch.
 * Paths, methods, params and bodies are checked against the generated
 * OpenAPI contract (schema.d.ts).
 *
 * Response parsing preserves the previous json-bigint behavior (large
 * integers arrive as strings) via a custom fetch wrapper. The
 * `X-Session-ID` header is attached per request from localStorage.
 *
 * With `USE_MOCK` set, requests are served by the in-repo mock layer
 * (`$lib/mock`) instead of the network, so the whole UI can be exercised
 * without a running backend.
 */

import { goto } from '$app/navigation';
import { page } from '$app/state';
import createClient from 'openapi-fetch';
import JSONBigint from 'json-bigint';
import type { paths } from './schema';
import { API_BASE_URL, USE_MOCK } from '$app/env/public';
import {
	MockFailure,
	failureBody,
	register,
	routeMock,
	type MockContext,
} from '$lib/mock/index';
import { connectionHandlers } from '$lib/mock/handlers/connection';
import { dataBrowserHandlers } from '$lib/mock/handlers/dataBrowser';
import { graphHandlers } from '$lib/mock/handlers/graph';
import { monitoringHandlers } from '$lib/mock/handlers/monitoring';
import { queryHandlers } from '$lib/mock/handlers/query';
import { schemaHandlers } from '$lib/mock/handlers/schema';

export const BASE_URL = API_BASE_URL;

/** Whether requests are served by the mock layer instead of the backend. */
export const isMockMode = USE_MOCK;

const JSONBig = JSONBigint({ storeAsString: true });

function handleUnauthorized() {
	try {
		localStorage.removeItem('sessionId');
	} catch {
		/* storage unavailable */
	}
	if (page.url.pathname !== '/login') void goto('/login');
}

/** fetch wrapper that parses JSON with json-bigint (large ints stay strings). */
async function bigintFetch(
	input: RequestInfo | URL,
	init?: RequestInit,
): Promise<Response> {
	const response = await fetch(input, init);
	if (response.status === 401) handleUnauthorized();
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
		headers: response.headers,
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

function failureMessage(
	error: unknown,
	status: number,
): { message: string; code?: string } {
	if (error && typeof error === 'object') {
		const body = error as FailureBody;
		if (typeof body.error === 'string' && body.error) {
			return { message: body.error };
		}
		if (body.error && typeof body.error === 'object') {
			if (body.error.message)
				return { message: body.error.message, code: body.error.code };
		}
		if (typeof body.message === 'string' && body.message)
			return { message: body.message };
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
	}>,
): Promise<T> {
	const res = await promise;
	if (res.error !== undefined && res.error !== null) {
		const { message, code } = failureMessage(
			res.error,
			res.response?.status ?? 0,
		);
		if (res.response?.status === 401) handleUnauthorized();
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
	if (
		!envelope.success ||
		envelope.data === undefined ||
		envelope.data === null
	) {
		throw new ApiError(
			envelope.error?.message ?? 'Request failed',
			0,
			envelope.error?.code,
		);
	}
	return envelope.data;
}

/** Mock-mode failure result shaped like an openapi-fetch response. */
function mockFailureResult(failure: MockFailure): {
	data?: undefined;
	error: unknown;
	response: Response;
} {
	if (failure.status === 401) handleUnauthorized();
	return {
		error: failureBody(failure.code, failure.message),
		response: new Response(
			JSON.stringify(failureBody(failure.code, failure.message)),
			{
				status: failure.status,
			},
		),
	};
}

/** Extract `{ path, query, body }` from an openapi-fetch options object. */
function mockContext(options?: {
	params?: { path?: Record<string, string>; query?: Record<string, unknown> };
	body?: unknown;
}): MockContext {
	return {
		path: options?.params?.path ?? {},
		query: (options?.params?.query ?? {}) as MockContext['query'],
		body: options?.body,
	};
}

type MockMethod = 'GET' | 'POST' | 'PUT' | 'DELETE' | 'PATCH';

function mockMethod(
	method: MockMethod,
	pathTemplate: string,
	options?: Parameters<typeof mockContext>[0],
) {
	const promise = routeMock(method, pathTemplate, mockContext(options))
		.then((result) => ({
			data: result.payload,
			error: undefined,
			response: new Response(JSON.stringify(result.payload), {
				status: result.status,
			}),
		}))
		.catch((failure: unknown) =>
			failure instanceof MockFailure
				? mockFailureResult(failure)
				: mockFailureResult(
						new MockFailure(500, 'MOCK_ERROR', String(failure)),
					),
		);
	return promise as Promise<{
		data?: unknown;
		error?: unknown;
		response: Response;
	}>;
}

const networkClient = createClient<paths>({
	baseUrl: BASE_URL,
	headers: { 'Content-Type': 'application/json' },
	fetch: bigintFetch,
});

/** Attach `X-Session-ID` on every request. */
networkClient.use({
	async onRequest({ request }) {
		try {
			const sessionId = localStorage.getItem('sessionId');
			if (sessionId) request.headers.set('X-Session-ID', sessionId);
		} catch {
			/* storage unavailable */
		}
		return request;
	},
});

/**
 * Mock proxy exposing the same method surface as the openapi-fetch client.
 * Dispatch is keyed on the exact path template literal the callers pass, so no
 * URL parsing is needed.
 */
const mockClient = {
	GET: (path: string, options?: Parameters<typeof mockContext>[0]) =>
		mockMethod('GET', path, options),
	POST: (path: string, options?: Parameters<typeof mockContext>[0]) =>
		mockMethod('POST', path, options),
	PUT: (path: string, options?: Parameters<typeof mockContext>[0]) =>
		mockMethod('PUT', path, options),
	DELETE: (path: string, options?: Parameters<typeof mockContext>[0]) =>
		mockMethod('DELETE', path, options),
	PATCH: (path: string, options?: Parameters<typeof mockContext>[0]) =>
		mockMethod('PATCH', path, options),
};

// Register all domain handlers once, at module load.
register({
	...connectionHandlers,
	...schemaHandlers,
	...graphHandlers,
	...queryHandlers,
	...dataBrowserHandlers,
	...monitoringHandlers,
});

export const client = (
	isMockMode ? mockClient : networkClient
) as typeof networkClient;
