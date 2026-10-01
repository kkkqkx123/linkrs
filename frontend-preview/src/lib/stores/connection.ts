import { writable, derived } from 'svelte/store';
import { storage } from '$utils/storage';
import { connectionService } from '$services/connection';
import { STORAGE_KEYS, DEFAULT_VALUES } from '$config/constants';

export interface ConnectionInfo {
  username: string;
  password?: string;
}

interface ConnectionState {
  isConnected: boolean;
  isVerified: boolean;
  connectionInfo: ConnectionInfo;
  sessionId: number | null;
  rememberMe: boolean;
  isLoading: boolean;
  error: string | null;
}

function createConnectionStore() {
  const saved = storage.get<{ connectionInfo: ConnectionInfo; rememberMe: boolean; isConnected: boolean; isVerified: boolean; sessionId: number | null }>('connection-storage');
  const { subscribe, set, update } = writable<ConnectionState>({
    isConnected: saved?.isConnected ?? false,
    isVerified: saved?.isVerified ?? false,
    connectionInfo: saved?.connectionInfo ?? { username: DEFAULT_VALUES.USERNAME },
    sessionId: saved?.sessionId ?? null,
    rememberMe: saved?.rememberMe ?? false,
    isLoading: false,
    error: null,
  });

  const persist = (state: ConnectionState) => {
    storage.set('connection-storage', {
      connectionInfo: state.connectionInfo,
      rememberMe: state.rememberMe,
      isConnected: state.isConnected,
      isVerified: state.isVerified,
      sessionId: state.sessionId,
    });
  };

  return {
    subscribe,
    login: async (username: string, password: string, rememberMe = false) => {
      update(s => ({ ...s, isLoading: true, error: null, isVerified: false }));
      try {
        const result = await connectionService.login({ username, password });
        const connectionInfo: ConnectionInfo = { username, password: rememberMe ? password : undefined };
        const newState = {
          isConnected: true, isVerified: true, connectionInfo,
          sessionId: result.session_id, rememberMe, isLoading: false, error: null,
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
        if (result.session_id) localStorage.setItem(STORAGE_KEYS.SESSION_ID, String(result.session_id));
      } catch (err: unknown) {
        const errorMessage = err instanceof Error ? err.message : 'Login failed';
        set({ isConnected: false, isVerified: false, sessionId: null, isLoading: false, error: errorMessage, connectionInfo: { username: DEFAULT_VALUES.USERNAME }, rememberMe: false });
        throw err;
      }
    },
    logout: async () => {
      update(s => ({ ...s, isLoading: true }));
      try {
        let currentState: ConnectionState = { isConnected: false, isVerified: false, connectionInfo: { username: '' }, sessionId: null, rememberMe: false, isLoading: false, error: null };
        update(s => { currentState = s; return s; });
        if (currentState.sessionId) await connectionService.logout(currentState.sessionId);
      } catch (error) { console.error('Logout error:', error); }
      finally {
        const emptyState = { isConnected: false, isVerified: false, sessionId: null, isLoading: false, connectionInfo: { username: DEFAULT_VALUES.USERNAME }, rememberMe: false, error: null };
        set(emptyState);
        persist(emptyState);
        localStorage.removeItem(STORAGE_KEYS.SESSION_ID);
      }
    },
    checkHealth: async () => {
      let currentState: ConnectionState = { isConnected: false, isVerified: false, connectionInfo: { username: '' }, sessionId: null, rememberMe: false, isLoading: false, error: null };
      update(s => { currentState = s; return s; });
      if (!currentState.isConnected || !currentState.sessionId) return false;
      try {
        const result = await connectionService.health();
        if (result.status !== 'healthy') {
          const emptyState = { isConnected: false, isVerified: false, sessionId: null, connectionInfo: { username: DEFAULT_VALUES.USERNAME }, rememberMe: false, isLoading: false, error: 'Connection lost' };
          set(emptyState);
          persist(emptyState);
          localStorage.removeItem(STORAGE_KEYS.SESSION_ID);
          return false;
        }
        update(s => ({ ...s, isVerified: true }));
        return true;
      } catch {
        const emptyState = { isConnected: false, isVerified: false, sessionId: null, connectionInfo: { username: DEFAULT_VALUES.USERNAME }, rememberMe: false, isLoading: false, error: 'Health check failed' };
        set(emptyState);
        persist(emptyState);
        localStorage.removeItem(STORAGE_KEYS.SESSION_ID);
        return false;
      }
    },
    clearError: () => update(s => ({ ...s, error: null })),
    loadSavedConnection: () => {
      const savedConnection = storage.get<ConnectionInfo>(STORAGE_KEYS.CONNECTION);
      const rememberMe = storage.get<boolean>(STORAGE_KEYS.REMEMBER_ME, false);
      if (savedConnection && rememberMe) {
        update(s => ({ ...s, connectionInfo: savedConnection, rememberMe: true }));
      }
    },
  };
}

export const connectionStore = createConnectionStore();
export const isAuthenticated = derived(connectionStore, $s => $s.isConnected && $s.isVerified);