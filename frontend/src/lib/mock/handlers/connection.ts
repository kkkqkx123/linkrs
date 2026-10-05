/**
 * Mock handlers for auth and session endpoints (`/v1/auth/**`, `/v1/health`,
 * `/v1/sessions**`).
 */

import { MockFailure, type MockEntry, type MockRegistry } from '../index';

const MOCK_SESSION_ID = 424242;

interface LoginBody {
	username?: unknown;
	password?: unknown;
}

export const connectionHandlers: MockRegistry = {
	'POST /v1/auth/login': ((ctx) => {
		const body = (ctx.body ?? {}) as LoginBody;
		if (body.password === 'wrong') {
			throw new MockFailure(
				401,
				'AUTH_FAILED',
				'Invalid username or password (mock fixture)',
			);
		}
		return {
			session_id: MOCK_SESSION_ID,
			username: typeof body.username === 'string' ? body.username : 'demo',
			expires_at: Math.floor(Date.now() / 1000) + 3600,
		};
	}) satisfies MockEntry,

	'POST /v1/auth/logout': { success: true } satisfies MockEntry,

	'GET /v1/health': {
		status: 'ok',
		version: '0.1.0',
		uptime: 86400,
	} satisfies MockEntry,

	'POST /v1/sessions': {
		session_id: MOCK_SESSION_ID,
		username: 'demo',
		space_name: 'social',
		graph_addr: 'mock://graph',
		timezone: 'UTC',
		created_at: Math.floor(Date.now() / 1000),
	} satisfies MockEntry,

	'GET /v1/sessions/{id}': ((ctx) => ({
		session_id: Number(ctx.path.id ?? MOCK_SESSION_ID),
		username: 'demo',
		space_name: 'social',
		graph_addr: 'mock://graph',
		timezone: 'UTC',
	})) satisfies MockEntry,

	'DELETE /v1/sessions/{id}': { success: true } satisfies MockEntry,
};
