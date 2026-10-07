/**
 * User management service with HTTP-first and query-channel fallback.
 *
 * New `/v1/users` endpoints are preferred when the backend provides them.
 * `SHOW USERS` and user DDL statements remain available so the users page
 * stays usable against older backends with limited fields.
 */
import { call, client } from '$lib/api/client';
import { queryService } from '$services/query';

export interface ManagedUser {
	username: string;
	role: string | null;
	status: string | null;
	lastActive: string | null;
}

export interface UserListResult {
	users: ManagedUser[];
	fallback: boolean;
}

function asRecord(value: unknown): Record<string, unknown> {
	return (value ?? {}) as Record<string, unknown>;
}

function text(value: unknown): string | null {
	return typeof value === 'string' && value ? value : null;
}

function normalizeUser(record: Record<string, unknown>): ManagedUser {
	return {
		username:
			text(record.username) ?? text(record.name) ?? text(record.user) ?? '',
		role:
			text(record.role) ??
			text(record.display_role) ??
			text(record.displayRole),
		status: text(record.status),
		lastActive:
			text(record.last_active) ??
			text(record.lastActive) ??
			text(record.last_active_at),
	};
}

function rowsFromQueryResult(data: unknown): ManagedUser[] {
	const record = asRecord(data);
	const rows = Array.isArray(record.rows)
		? (record.rows as Array<Record<string, unknown>>)
		: [];
	return rows.map((row) => normalizeUser(row)).filter((user) => user.username);
}

function quoteLiteral(value: string): string {
	return `'${value.replace(/'/g, "''")}'`;
}

function quoteIdent(value: string): string {
	if (/^[A-Za-z_][A-Za-z0-9_]*$/.test(value)) return value;
	return `"${value.replace(/"/g, '')}"`;
}

async function listViaQuery(): Promise<ManagedUser[]> {
	const response = await queryService.execute({ query: 'SHOW USERS' });
	if (!response.success || !response.data) {
		throw new Error(response.error?.message ?? 'SHOW USERS failed');
	}
	return rowsFromQueryResult(response.data);
}

async function runAdminStatement(query: string): Promise<void> {
	const response = await queryService.execute({ query });
	if (!response.success) {
		throw new Error(response.error?.message ?? 'Statement failed');
	}
}

function untypedGet(path: string) {
	const untyped = client as unknown as {
		GET: (
			path: string,
		) => Promise<{ data?: unknown; error?: unknown; response?: Response }>;
	};
	return call<unknown>(untyped.GET(path));
}

interface UntypedOptions {
	body?: unknown;
	params?: { path?: Record<string, string> };
}

function untypedPost(
	path: string,
	body: unknown,
	params?: Record<string, string>,
) {
	const untyped = client as unknown as {
		POST: (
			path: string,
			options?: UntypedOptions,
		) => Promise<{ data?: unknown; error?: unknown; response?: Response }>;
	};
	return call<unknown>(
		params
			? untyped.POST(path, { body, params: { path: params } })
			: untyped.POST(path, { body }),
	);
}

function untypedDelete(path: string, params?: Record<string, string>) {
	const untyped = client as unknown as {
		DELETE: (
			path: string,
			options?: UntypedOptions,
		) => Promise<{ data?: unknown; error?: unknown; response?: Response }>;
	};
	return call<unknown>(
		params
			? untyped.DELETE(path, { params: { path: params } })
			: untyped.DELETE(path),
	);
}

export const usersService = {
	list: async (): Promise<UserListResult> => {
		try {
			const payload = await untypedGet('/v1/users');
			const record = asRecord(payload);
			const raw = Array.isArray(record.users)
				? (record.users as Array<Record<string, unknown>>)
				: Array.isArray(record.data)
					? (record.data as Array<Record<string, unknown>>)
					: [];
			return { users: raw.map(normalizeUser), fallback: false };
		} catch {
			return { users: await listViaQuery(), fallback: true };
		}
	},

	create: async (username: string, password: string): Promise<void> => {
		try {
			await untypedPost('/v1/users', { username, password });
		} catch {
			await runAdminStatement(
				`CREATE USER ${quoteIdent(username)} WITH PASSWORD ${quoteLiteral(password)}`,
			);
		}
	},

	resetPassword: async (username: string, password: string): Promise<void> => {
		try {
			await untypedPost(
				'/v1/users/{name}/password',
				{ password },
				{ name: username },
			);
		} catch {
			await runAdminStatement(
				`ALTER USER ${quoteIdent(username)} WITH PASSWORD ${quoteLiteral(password)}`,
			);
		}
	},

	setEnabled: async (username: string, enabled: boolean): Promise<void> => {
		await untypedPost(
			'/v1/users/{name}/' + (enabled ? 'enable' : 'disable'),
			{},
			{ name: username },
		);
	},

	grant: async (
		username: string,
		role: string,
		space: string,
	): Promise<void> => {
		const normalizedRole = role.toUpperCase();
		try {
			await untypedPost(
				'/v1/users/{name}/grant',
				{ role: normalizedRole, space },
				{ name: username },
			);
		} catch {
			await runAdminStatement(
				`GRANT ${normalizedRole} ON ${quoteIdent(space)} TO ${quoteIdent(username)}`,
			);
		}
	},

	revoke: async (
		username: string,
		role: string,
		space: string,
	): Promise<void> => {
		const normalizedRole = role.toUpperCase();
		try {
			await untypedPost(
				'/v1/users/{name}/revoke',
				{ space },
				{ name: username },
			);
		} catch {
			await runAdminStatement(
				`REVOKE ${normalizedRole} ON ${quoteIdent(space)} FROM ${quoteIdent(username)}`,
			);
		}
	},

	drop: async (username: string): Promise<void> => {
		try {
			await untypedDelete('/v1/users/{name}', { name: username });
		} catch {
			await runAdminStatement(`DROP USER ${quoteIdent(username)}`);
		}
	},
};

export default usersService;
