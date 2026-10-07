/**
 * UI role model and capability gates for login and permission views.
 *
 * The backend keeps five roles while the UI shows three. Missing role
 * information means a legacy single-user backend and grants full access
 * so existing deployments keep working. Explicit unknown values fall
 * back to the read-only role.
 */

export type UiRole = 'admin' | 'operator' | 'viewer';

export type Capability =
	'manageUsers' | 'write' | 'alterSchema' | 'dropSpace' | 'manageConfig';

const RANK: Record<UiRole, number> = {
	viewer: 0,
	operator: 1,
	admin: 2,
};

/** Map one backend role name to a UI role. Null means legacy backend. */
export function toUiRole(backendRole?: string | null): UiRole | null {
	if (backendRole === undefined || backendRole === null) return null;
	switch (backendRole.toUpperCase()) {
		case 'GOD':
		case 'ADMIN':
			return 'admin';
		case 'DBA':
		case 'USER':
			return 'operator';
		case 'GUEST':
			return 'viewer';
		case '':
			return null;
		default:
			return 'viewer';
	}
}

/** Pick the highest privilege UI role from backend role names. */
export function highestUiRole(
	roles?: Array<string | null | undefined> | null,
): UiRole | null {
	if (!roles || roles.length === 0) return null;
	let best: UiRole | null = null;
	for (const raw of roles) {
		const mapped = toUiRole(raw);
		if (mapped === null) continue;
		if (best === null || RANK[mapped] > RANK[best]) best = mapped;
	}
	return best;
}

/** Extract backend role names from a login or me payload without typing. */
export function rolesFromPayload(value: unknown): string[] {
	if (!value || typeof value !== 'object') return [];
	const record = value as Record<string, unknown>;
	const out: string[] = [];
	const push = (item: unknown) => {
		if (typeof item === 'string' && item) out.push(item);
	};
	push(record.role);
	push(record.display_role);
	push(record.displayRole);
	const roles = record.roles;
	if (Array.isArray(roles)) {
		for (const entry of roles) {
			if (typeof entry === 'string') push(entry);
			else if (entry && typeof entry === 'object') {
				const item = entry as Record<string, unknown>;
				push(item.role);
			}
		}
	}
	const spaceRoles = record.space_roles ?? record.spaceRoles;
	if (
		spaceRoles &&
		typeof spaceRoles === 'object' &&
		!Array.isArray(spaceRoles)
	) {
		for (const item of Object.values(spaceRoles as Record<string, unknown>)) {
			push(item);
		}
	}
	return out;
}

/** Resolve the effective UI role. Null preserves legacy full access. */
export function resolveUiRole(
	value: unknown,
	fallbackUsername?: string,
): UiRole | null {
	const names = rolesFromPayload(value);
	if (names.length > 0) return highestUiRole(names) ?? 'viewer';
	if (fallbackUsername === 'root') return 'admin';
	if (fallbackUsername) return 'admin';
	return null;
}

/** Central capability check used at store action entries. */
export function can(capability: Capability, role: UiRole | null): boolean {
	if (role === null) return true;
	switch (capability) {
		case 'manageUsers':
			return role === 'admin';
		case 'write':
			return role === 'admin' || role === 'operator';
		case 'alterSchema':
			return role === 'admin' || role === 'operator';
		case 'dropSpace':
			return role === 'admin';
		case 'manageConfig':
			return role === 'admin' || role === 'operator';
		default:
			return false;
	}
}
