import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import { storage } from '@/utils/storage';
import { connectionService } from '@/services/connection';
import { STORAGE_KEYS, DEFAULT_VALUES } from '@/utils/constants';

export interface ConnectionInfo {
  username: string;
  password?: string;
}

export interface ConnectionState {
  isConnected: boolean;
  isVerified: boolean;
  connectionInfo: ConnectionInfo;
  sessionId: number | null;
  rememberMe: boolean;
  isLoading: boolean;
  error: string | null;
  login: (username: string, password: string, rememberMe?: boolean) => Promise<void>;
  logout: () => Promise<void>;
  checkHealth: () => Promise<boolean>;
  clearError: () => void;
  loadSavedConnection: () => void;
}

export const useConnectionStore = create<ConnectionState>()(
  persist(
    (set, get) => ({
      isConnected: false,
      isVerified: false,
      connectionInfo: {
        username: DEFAULT_VALUES.USERNAME,
      },
      sessionId: null,
      rememberMe: false,
      isLoading: false,
      error: null,

      login: async (username: string, password: string, rememberMe = false) => {
        set({ isLoading: true, error: null, isVerified: false });
        try {
          const result = await connectionService.login({ username, password });

          const connectionInfo: ConnectionInfo = {
            username,
            password: rememberMe ? password : undefined,
          };

          set({
            isConnected: true,
            isVerified: true,
            connectionInfo,
            sessionId: result.session_id,
            rememberMe,
            isLoading: false,
          });

          if (rememberMe) {
            storage.set(STORAGE_KEYS.CONNECTION, connectionInfo);
            storage.set(STORAGE_KEYS.REMEMBER_ME, true);
          } else {
            storage.remove(STORAGE_KEYS.CONNECTION);
            storage.set(STORAGE_KEYS.REMEMBER_ME, false);
          }

          if (result.session_id) {
            localStorage.setItem(STORAGE_KEYS.SESSION_ID, String(result.session_id));
          }
        } catch (err: unknown) {
          const errorMessage = err instanceof Error ? err.message : 'Login failed';
          set({
            isConnected: false,
            isVerified: false,
            sessionId: null,
            isLoading: false,
            error: errorMessage,
          });
          throw err;
        }
      },

      logout: async () => {
        set({ isLoading: true });
        try {
          const { sessionId } = get();
          if (sessionId) {
            await connectionService.logout(sessionId);
          }
        } catch (error) {
          console.error('Logout error:', error);
        } finally {
          set({
            isConnected: false,
            isVerified: false,
            sessionId: null,
            isLoading: false,
            connectionInfo: {
              username: DEFAULT_VALUES.USERNAME,
            },
          });
          localStorage.removeItem(STORAGE_KEYS.SESSION_ID);
        }
      },

      checkHealth: async () => {
        const { isConnected, sessionId } = get();
        if (!isConnected || !sessionId) {
          return false;
        }

        try {
          const result = await connectionService.health();
          if (result.status !== 'healthy') {
            set({
              isConnected: false,
              isVerified: false,
              sessionId: null,
              error: 'Connection lost',
            });
            localStorage.removeItem(STORAGE_KEYS.SESSION_ID);
            return false;
          }
          set({ isVerified: true });
          return true;
        } catch {
          set({
            isConnected: false,
            isVerified: false,
            sessionId: null,
            error: 'Health check failed',
          });
          localStorage.removeItem(STORAGE_KEYS.SESSION_ID);
          return false;
        }
      },

      clearError: () => {
        set({ error: null });
      },

      loadSavedConnection: () => {
        const savedConnection = storage.get<ConnectionInfo>(STORAGE_KEYS.CONNECTION);
        const rememberMe = storage.get<boolean>(STORAGE_KEYS.REMEMBER_ME, false);

        if (savedConnection && rememberMe) {
          set({
            connectionInfo: savedConnection,
            rememberMe: true,
          });
        }
      },
    }),
    {
      name: 'connection-storage',
      partialize: (state) => ({
        connectionInfo: state.connectionInfo,
        rememberMe: state.rememberMe,
        isConnected: state.isConnected,
        isVerified: state.isVerified,
        sessionId: state.sessionId,
      }),
    }
  )
);
