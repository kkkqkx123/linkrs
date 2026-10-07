import { call, client } from '$lib/api/client';
import type { components } from '$lib/api/schema';
import { asArray, isRecord } from '$utils/parse';

type LoginRequest = components['schemas']['LoginRequest'];
type LoginResponse = components['schemas']['LoginResponse'];
type LogoutRequest = components['schemas']['LogoutRequest'];
type CreateSessionRequest = components['schemas']['CreateSessionRequest'];
type SessionResponse = components['schemas']['SessionResponse'];

/** Health payload; the contract leaves it untyped so the view narrows it. */
export interface HealthResponse {
	status?: string;
	version?: string;
	uptime?: number;
	[key: string]: unknown;
}

export interface LoginParams {
	username: string;
	password: string;
}

export interface CreateSessionParams {
	username: string;
	clientIp: string;
}

/** Current user payload; extra role fields stay optional until backends adopt them. */
export interface AuthMeResponse {
	username: string;
	role?: string | null;
	display_role?: string | null;
	roles?: Array<string | { role?: string }> | null;
	space_roles?: Record<string, string> | null;
	[key: string]: unknown;
}

/** Session detail; the contract leaves it untyped so the view narrows it. */
export interface SessionDetail {
	session_id: number;
	username: string;
	space_name?: string;
	graph_addr?: string;
	timezone?: string;
}

function asSessionDetail(value: unknown): SessionDetail {
	const record = isRecord(value) ? value : {};
	return {
		session_id: typeof record.session_id === 'number' ? record.session_id : 0,
		username: typeof record.username === 'string' ? record.username : '',
		space_name:
			typeof record.space_name === 'string' ? record.space_name : undefined,
		graph_addr:
			typeof record.graph_addr === 'string' ? record.graph_addr : undefined,
		timezone: typeof record.timezone === 'string' ? record.timezone : undefined,
	};
}

export interface SessionListItem {
	session_id: number;
	username: string;
	space_name?: string;
	graph_addr?: string;
	active_queries?: number;
}

export interface SessionListResult {
	sessions: SessionListItem[];
}

function asSessionList(value: unknown): SessionListItem[] {
	const record = isRecord(value) ? value : {};
	const raw = Array.isArray(record.sessions) ? record.sessions : [];
	return asArray(raw).map((entry) => ({
		session_id: typeof entry.session_id === 'number' ? entry.session_id : 0,
		username: typeof entry.username === 'string' ? entry.username : '',
		space_name:
			typeof entry.space_name === 'string' ? entry.space_name : undefined,
		graph_addr:
			typeof entry.graph_addr === 'string' ? entry.graph_addr : undefined,
		active_queries:
			typeof entry.active_queries === 'number'
				? entry.active_queries
				: undefined,
	}));
}

export const connectionService = {
	login: async (params: LoginParams): Promise<LoginResponse> =>
		call(client.POST('/v1/auth/login', { body: params as LoginRequest })),

	logout: async (sessionId: number): Promise<void> => {
		await call<unknown>(
			client.POST('/v1/auth/logout', {
				body: { session_id: sessionId } as LogoutRequest,
			}),
		);
	},

	health: async (): Promise<HealthResponse> => call(client.GET('/v1/health')),

	me: async (): Promise<AuthMeResponse> =>
		call(client.GET('/v1/auth/me')),

	sessions: {
		create: async (params: CreateSessionParams): Promise<SessionResponse> =>
			call(
				client.POST('/v1/sessions', {
					body: {
						username: params.username,
						client_ip: params.clientIp,
					} as CreateSessionRequest,
				}),
			),
		list: async (): Promise<SessionListResult> =>
			({
				sessions: asSessionList(await call<unknown>(client.GET('/v1/sessions'))),
			}),
		get: async (id: number): Promise<SessionDetail> =>
			asSessionDetail(
				await call<unknown>(
					client.GET('/v1/sessions/{id}', { params: { path: { id } } }),
				),
			),
		delete: async (id: number): Promise<void> => {
			await call<unknown>(
				client.DELETE('/v1/sessions/{id}', { params: { path: { id } } }),
			);
		},
	},
};

export default connectionService;
