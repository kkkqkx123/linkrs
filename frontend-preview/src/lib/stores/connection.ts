import { writable, derived } from 'svelte/store';
import { storage } from '$utils/storage';
import { connectionService } from '$services/connection';
import { STORAGE_KEYS, DEFAULT_VALUES } from '$config/constants';
import {
	can,
	resolveUiRole,
	type Capability,
	type UiRole,
} from '$lib/auth/roles';
import { t } from '$i18n';

export interface ConnectionInfo {
	username: string;
}

interface ConnectionState {
	isConnected: boolean;
	isVerified: boolean;
	connectionInfo: ConnectionInfo;
	sessionId: number | null;
	role: UiRole | null;
	mustChangePassword: boolean;
	expiresAt: number | null;
	rememberMe: boolean;
	isLoading: boolean;
	error: string | null;
}

function expiresFromPayload(value: unknown): number | null {
	if (!value || typeof value !== 'object') return null;
	const record = value as Record<string, unknown>;
	const raw = record.expires_at ?? record.expiresAt;
	if (typeof raw === 'number' && Number.isFinite(raw)) return raw;
	if (typeof raw === 'string' && /^\d+$/.test(raw)) return Number(raw);
	return null;
}

let expiryTimer: ReturnType<typeof setTimeout> | null = null;

function clearExpiryTimer() {
	if (expiryTimer !== null) {
		clearTimeout(expiryTimer);
		expiryTimer = null;
	}
}

function mustChangeFromPayload(value: unknown): boolean {
	if (!value || typeof value !== 'object') return false;
	const record = value as Record<string, unknown>;
	return (
		record.must_change_password === true ||
		record.mustChangePassword === true
	);
}

async function resolveRoleAfterLogin(
	result: unknown,
	username: string,
): Promise<UiRole | null> {
	const fromLogin = resolveUiRole(result, username);
	try {
		const me = await connectionService.me();
		const fromMe = resolveUiRole(me, username);
		return fromMe ?? fromLogin;
	} catch {
		return fromLogin;
	}
}

function createConnectionStore() {
	const saved = storage.get<{
		connectionInfo: ConnectionInfo;
		rememberMe: boolean;
		isConnected: boolean;
		isVerified: boolean;
		sessionId: number | null;
		role: UiRole | null;
		mustChangePassword: boolean;
		expiresAt: number | null;
	}>('connection-storage');
	const savedExpired =
		typeof saved?.expiresAt === 'number' && saved.expiresAt * 1000 <= Date.now();
	const { subscribe, set, update } = writable<ConnectionState>({
		isConnected: savedExpired ? false : (saved?.isConnected ?? false),
		isVerified: savedExpired ? false : (saved?.isVerified ?? false),
		connectionInfo: saved?.connectionInfo
			? { username: saved.connectionInfo.username || DEFAULT_VALUES.USERNAME }
			: {
					username: DEFAULT_VALUES.USERNAME,
				},
		sessionId: savedExpired ? null : (saved?.sessionId ?? null),
		role: savedExpired ? null : (saved?.role ?? null),
		mustChangePassword: savedExpired ? false : (saved?.mustChangePassword ?? false),
		expiresAt: savedExpired ? null : (saved?.expiresAt ?? null),
		rememberMe: saved?.rememberMe ?? false,
		isLoading: false,
		error: null,
	});

	const persist = (state: ConnectionState) => {
		storage.set('connection-storage', {
			connectionInfo: { username: state.connectionInfo.username },
			rememberMe: state.rememberMe,
			isConnected: state.isConnected,
			isVerified: state.isVerified,
			sessionId: state.sessionId,
			role: state.role,
			mustChangePassword: state.mustChangePassword,
			expiresAt: state.expiresAt,
		});
	};

	const scheduleExpiryLogout = (expiresAt: number | null, logout: () => void) => {
		clearExpiryTimer();
		if (expiresAt === null) return;
		const delay = expiresAt * 1000 - Date.now();
		if (delay <= 0) {
			void logout();
			return;
		}
		expiryTimer = setTimeout(() => void logout(), Math.min(delay, 2_147_483_647));
	};

	const doLogout = async () => {
		clearExpiryTimer();
		update((s) => ({ ...s, isLoading: true }));
		try {
			let currentState: ConnectionState = {
				isConnected: false,
				isVerified: false,
				connectionInfo: { username: '' },
				sessionId: null,
				role: null,
				mustChangePassword: false,
				expiresAt: null,
				rememberMe: false,
				isLoading: false,
				error: null,
			};
			update((s) => {
				currentState = s;
				return s;
			});
			if (currentState.sessionId) await connectionService.logout();
		} catch (error) {
			console.error('Logout error:', error);
		} finally {
			const emptyState = {
				isConnected: false,
				isVerified: false,
				sessionId: null,
				role: null,
				mustChangePassword: false,
				expiresAt: null,
				isLoading: false,
				connectionInfo: { username: DEFAULT_VALUES.USERNAME },
				rememberMe: false,
				error: null,
			};
			set(emptyState);
			persist(emptyState);
			localStorage.removeItem(STORAGE_KEYS.SESSION_ID);
		}
	};

	if (!savedExpired && saved?.sessionId && typeof saved.expiresAt === 'number') {
		scheduleExpiryLogout(saved.expiresAt, doLogout);
	}

	return {
		subscribe,
		login: async (username: string, password: string, rememberMe = false) => {
			update((s) => ({
				...s,
				isLoading: true,
				error: null,
				isVerified: false,
			}));
			try {
				const result = await connectionService.login({ username, password });
				if (result.session_id)
					localStorage.setItem(
						STORAGE_KEYS.SESSION_ID,
						String(result.session_id),
					);
				const role = await resolveRoleAfterLogin(result, username);
				const connectionInfo: ConnectionInfo = { username };
				const expiresAt = expiresFromPayload(result);
				const newState = {
					isConnected: true,
					isVerified: true,
					connectionInfo,
					sessionId: result.session_id,
					role,
					mustChangePassword: mustChangeFromPayload(result),
					expiresAt,
					rememberMe,
					isLoading: false,
					error: null,
				};
				set(newState);
				persist(newState);
				scheduleExpiryLogout(expiresAt, doLogout);
				if (rememberMe) {
					storage.set(STORAGE_KEYS.CONNECTION, connectionInfo);
					storage.set(STORAGE_KEYS.REMEMBER_ME, true);
				} else {
					storage.remove(STORAGE_KEYS.CONNECTION);
					storage.set(STORAGE_KEYS.REMEMBER_ME, false);
				}
			} catch (err: unknown) {
				const errorMessage =
					err instanceof Error ? err.message : t('errors.loginFailed');
				localStorage.removeItem(STORAGE_KEYS.SESSION_ID);
				clearExpiryTimer();
				set({
					isConnected: false,
					isVerified: false,
					sessionId: null,
					role: null,
					mustChangePassword: false,
					expiresAt: null,
					isLoading: false,
					error: errorMessage,
					connectionInfo: { username: DEFAULT_VALUES.USERNAME },
					rememberMe: false,
				});
				throw err;
			}
		},
		refreshRole: async () => {
			try {
				const me = await connectionService.me();
				const role = resolveUiRole(me, me.username);
				update((s) => {
					const next = { ...s, role };
					persist(next);
					return next;
				});
				return role;
			} catch {
				return null;
			}
		},
		can: (capability: Capability, role: UiRole | null) => can(capability, role),
		logout: doLogout,
		checkHealth: async () => {
			let currentState: ConnectionState = {
				isConnected: false,
				isVerified: false,
				connectionInfo: { username: '' },
				sessionId: null,
				role: null,
				mustChangePassword: false,
				expiresAt: null,
				rememberMe: false,
				isLoading: false,
				error: null,
			};
			update((s) => {
				currentState = s;
				return s;
			});
			if (!currentState.isConnected || !currentState.sessionId) return false;
			try {
				const me = await connectionService.me();
				const role = resolveUiRole(me, me.username);
				update((s) => {
					const next = { ...s, isVerified: true, role };
					persist(next);
					return next;
				});
				return true;
			} catch {
				const emptyState = {
					isConnected: false,
					isVerified: false,
					sessionId: null,
					role: null,
					mustChangePassword: false,
					expiresAt: null,
					connectionInfo: { username: DEFAULT_VALUES.USERNAME },
					rememberMe: false,
					isLoading: false,
					error: t('errors.connectionLost'),
				};
				set(emptyState);
				persist(emptyState);
				clearExpiryTimer();
				localStorage.removeItem(STORAGE_KEYS.SESSION_ID);
				return false;
			}
		},
		clearError: () => update((s) => ({ ...s, error: null })),
		loadSavedConnection: () => {
			const savedConnection = storage.get<ConnectionInfo>(
				STORAGE_KEYS.CONNECTION,
			);
			const rememberMe = storage.get<boolean>(STORAGE_KEYS.REMEMBER_ME, false);
			if (savedConnection && rememberMe && savedConnection.username) {
				update((s) => ({
					...s,
					connectionInfo: { username: savedConnection.username },
					rememberMe: true,
				}));
			}
		},
	};
}

export const connectionStore = createConnectionStore();
export const isAuthenticated = derived(
	connectionStore,
	($s) => $s.isConnected && $s.isVerified,
);
export const currentRole = derived(connectionStore, ($s) => $s.role);
export const canManageUsers = derived(connectionStore, ($s) =>
	can('manageUsers', $s.role),
);
export const canWrite = derived(connectionStore, ($s) => can('write', $s.role));
