import { post, get, _delete } from '$utils/http';
import type { LoginRequest, LogoutRequest } from '$types/schema';

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

export interface LoginResponse {
  session_id: number;
  username: string;
  expires_at?: number;
}

export interface CreateSessionParams {
  username: string;
  client_ip?: string;
}

export interface CreateSessionResponse {
  session_id: number;
  username: string;
  created_at: number;
}

export interface SessionDetail {
  session_id: number;
  username: string;
  space_name?: string;
  graph_addr?: string;
  timezone?: string;
}

export const connectionService = {
  login: async (params: LoginParams): Promise<LoginResponse> => {
    return await post<LoginResponse>('/v1/auth/login', params as LoginRequest);
  },

  logout: async (sessionId: number): Promise<void> => {
    await post<void>('/v1/auth/logout', { session_id: sessionId } as LogoutRequest);
  },

  health: async (): Promise<HealthResponse> => {
    return await get<HealthResponse>('/v1/health');
  },

  sessions: {
    create: async (params: CreateSessionParams): Promise<CreateSessionResponse> => {
      return await post<CreateSessionResponse>('/v1/sessions', params);
    },
    get: async (id: number): Promise<SessionDetail> => {
      return await get<SessionDetail>(`/v1/sessions/${id}`);
    },
    delete: async (id: number): Promise<void> => {
      await _delete<void>(`/v1/sessions/${id}`);
    },
  },
};

export default connectionService;
