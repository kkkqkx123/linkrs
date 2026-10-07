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
	rememberMe: boolean;
	isLoading: boolean;
	error: string | null;
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
	}>('connection-storage');
	const { subscribe, set, update } = writable<ConnectionState>({
		isConnected: saved?.isConnected ?? false,
		isVerified: saved?.isVerified ?? false,
		connectionInfo: saved?.connectionInfo
			? { username: saved.connectionInfo.username || DEFAULT_VALUES.USERNAME }
			: {
					username: DEFAULT_VALUES.USERNAME,
				},
		sessionId: saved?.sessionId ?? null,
		role: saved?.role ?? null,
		mustChangePassword: saved?.mustChangePassword ?? false,
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
		});
	};

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
				const newState = {
					isConnected: true,
					isVerified: true,
					connectionInfo,
					sessionId: result.session_id,
					role,
					mustChangePassword: mustChangeFromPayload(result),
					rememberMe,
					isLoading: false,
					error: null,
				};
				set(newState);
				persist(newState);
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
				set({
					isConnected: false,
					isVerified: false,
					sessionId: null,
					role: null,
					mustChangePassword: false,
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
		logout: async () => {
			update((s) => ({ ...s, isLoading: true }));
			try {
				let currentState: ConnectionState = {
					isConnected: false,
					isVerified: false,
					connectionInfo: { username: '' },
					sessionId: null,
					role: null,
					mustChangePassword: false,
					rememberMe: false,
					isLoading: false,
					error: null,
				};
				update((s) => {
					currentState = s;
					return s;
				});
				if (currentState.sessionId)
					await connectionService.logout(currentState.sessionId);
			} catch (error) {
				console.error('Logout error:', error);
			} finally {
				const emptyState = {
					isConnected: false,
					isVerified: false,
					sessionId: null,
					role: null,
					mustChangePassword: false,
					isLoading: false,
					connectionInfo: { username: DEFAULT_VALUES.USERNAME },
					rememberMe: false,
					error: null,
				};
				set(emptyState);
				persist(emptyState);
				localStorage.removeItem(STORAGE_KEYS.SESSION_ID);
			}
		},
		checkHealth: async () => {
			let currentState: ConnectionState = {
				isConnected: false,
				isVerified: false,
				connectionInfo: { username: '' },
				sessionId: null,
				role: null,
				mustChangePassword: false,
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
					connectionInfo: { username: DEFAULT_VALUES.USERNAME },
					rememberMe: false,
					isLoading: false,
					error: t('errors.connectionLost'),
				};
				set(emptyState);
				persist(emptyState);
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
