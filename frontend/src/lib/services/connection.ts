import { post, get, _delete } from '$utils/http';

export interface LoginParams {
  username: string;
  password: string;
}

export interface LoginResponse {
  session_id: number;
  username: string;
  expires_at?: number;
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
    return await post('/v1/auth/login')(params) as LoginResponse;
  },

  logout: async (sessionId: number): Promise<void> => {
    await post('/v1/auth/logout')({ session_id: sessionId });
  },

  health: async (): Promise<HealthResponse> => {
    return await get('/v1/health')() as HealthResponse;
  },

  sessions: {
    create: async (params: CreateSessionParams): Promise<CreateSessionResponse> => {
      return await post('/v1/sessions')(params) as CreateSessionResponse;
    },
    get: async (id: number): Promise<SessionDetail> => {
      return await get(`/v1/sessions/${id}`)() as SessionDetail;
    },
    delete: async (id: number): Promise<void> => {
      await _delete(`/v1/sessions/${id}`)();
    },
  },
};