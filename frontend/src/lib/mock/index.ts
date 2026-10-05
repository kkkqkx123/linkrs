/**
 * Mock request router, active when the app runs with `USE_MOCK` set.
 *
 * Handlers are registered under `"<METHOD> <path-template>"` keys using the
 * exact OpenAPI path templates the services pass to the client (e.g.
 * `"GET /api/v1/schema/spaces/{name}/details"`), so lookups are exact string
 * matches — no regex parsing. `/api/v1/**` web endpoints return the standard
 * `{ success, data, error }` envelope because their callers use `unwrap()`;
 * `/v1/**` endpoints return the bare payload.
 */

import { mockDelay, scenarioIsEmpty, scenarioWantsError } from './scenario';

export interface MockContext {
	path: Record<string, string>;
	query: Record<string, string | string[] | undefined>;
	body?: unknown;
}

export type MockHandler = (ctx: MockContext) => unknown | Promise<unknown>;

/** Registry value: either a dynamic handler or a static payload. */
export type MockEntry =
	| MockHandler
	| Record<string, unknown>
	| MockEnvelope<unknown>
	| unknown[]
	| string
	| number
	| boolean
	| null;

/** Envelope shape shared by `/api/v1` web endpoints (mirrors api/client.ts). */
export interface MockEnvelope<T> {
	success: boolean;
	data?: T | null;
	error?: { code: string; message: string } | null;
}

export function envelope<T>(data: T): MockEnvelope<T> {
	return { success: true, data };
}

export function emptyEnvelope(): MockEnvelope<null> {
	return { success: true, data: scenarioIsEmpty() ? null : null };
}

/** Typed registry: `"<METHOD> <template>"` -> handler or static payload. */
export type MockRegistry = Record<string, MockEntry>;

const registry: MockRegistry = {};

export function register(handlers: MockRegistry) {
	for (const [key, entry] of Object.entries(handlers)) {
		if (key in registry) {
			throw new Error(`[Mock] Duplicate handler registered for ${key}`);
		}
		registry[key] = entry;
	}
}

export interface MockResult {
	status: number;
	payload: unknown;
}

/**
 * Route a mocked request. Returns the payload on success, or throws a
 * `MockFailure` carrying the HTTP status and error body.
 */
export async function routeMock(
	method: string,
	pathTemplate: string,
	ctx: MockContext,
): Promise<MockResult> {
	if (scenarioWantsError()) {
		throw new MockFailure(
			503,
			'SERVICE_UNAVAILABLE',
			'Mock error scenario is active',
		);
	}
	const entry = registry[`${method} ${pathTemplate}`];
	if (entry === undefined) {
		console.warn(
			`[Mock] Unhandled ${method} ${pathTemplate} — add a fixture in src/lib/mock`,
		);
		throw new MockFailure(
			404,
			'NOT_FOUND',
			`No mock for ${method} ${pathTemplate}`,
		);
	}
	await mockDelay();
	const payload =
		typeof entry === 'function' ? await (entry as MockHandler)(ctx) : entry;
	return { status: 200, payload };
}

/** Error thrown by handlers and by the router for failure responses. */
export class MockFailure extends Error {
	constructor(
		public status: number,
		public code: string,
		message: string,
	) {
		super(message);
		this.name = 'MockFailure';
	}
}

/** Build the standard failure body emitted by the backend. */
export function failureBody(code: string, message: string): unknown {
	return { error: { code, message } };
}
