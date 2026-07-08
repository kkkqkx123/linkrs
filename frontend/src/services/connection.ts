import { post, get, _delete } from '@/utils/http';

export interface LoginParams {
  username: string;
  password: string;
}

export interface LoginResponse {
  session_id: number;
  username: string;
  expires_at?: number;
}

export interface LogoutParams {
  session_id: number;
}

export interface HealthResponse {
  status: string;
  service: string;
  version: string;
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
    const response = await post('/v1/auth/login')(params) as LoginResponse;
    return response;
  },

  logout: async (sessionId: number): Promise<void> => {
    await post('/v1/auth/logout')({ session_id: sessionId });
  },

  health: async (): Promise<HealthResponse> => {
    const response = await get('/v1/health')() as HealthResponse;
    return response;
  },

  // Session Management APIs
  sessions: {
    create: async (params: CreateSessionParams): Promise<CreateSessionResponse> => {
      const response = await post('/v1/sessions')(params) as CreateSessionResponse;
      return response;
    },

    get: async (id: number): Promise<SessionDetail> => {
      const response = await get(`/v1/sessions/${id}`)() as SessionDetail;
      return response;
    },

    delete: async (id: number): Promise<void> => {
      await _delete(`/v1/sessions/${id}`)();
    },
  },
};
