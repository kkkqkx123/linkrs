/**
 * Mock handlers for user management endpoints (`/v1/users**`).
 * New backends serve these directly; older backends fall back to the
 * query channel, so fixtures mirror the HTTP-first contract.
 */
import type { MockRegistry } from '../index';

export const usersHandlers: MockRegistry = {
	'GET /v1/users': {
		users: [
			{
				username: 'root',
				role: 'GOD',
				status: 'enabled',
				last_active: '2026-10-07T00:00:00Z',
			},
			{
				username: 'demo',
				role: 'ADMIN',
				status: 'enabled',
				last_active: '2026-10-07T00:00:00Z',
			},
		],
	},
	'POST /v1/users': { success: true },
	'POST /v1/users/{name}/password': { success: true },
	'POST /v1/users/{name}/enable': { success: true },
	'POST /v1/users/{name}/disable': { success: true },
	'POST /v1/users/{name}/grant': { success: true },
	'POST /v1/users/{name}/revoke': { success: true },
	'DELETE /v1/users/{name}': { success: true },
};
