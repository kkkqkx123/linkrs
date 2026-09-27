import { post, get, _delete } from '$utils/http';
import type { HistoryItem, HistoryParams, FavoriteItem, FavoriteParams, UpdateFavoriteParams } from '$types/query';
import type { ApiResponse_HistoryListResponse, ApiResponse_FavoriteListResponse, AddHistoryRequest } from '$types/schema';

export const queryHistoryService = {
  history: {
    add: async (params: Omit<AddHistoryRequest, 'query'>): Promise<HistoryItem> =>
      await post<HistoryItem>('/api/v1/queries/history', params as AddHistoryRequest),
    list: async (): Promise<HistoryItem[]> => 
      await get<ApiResponse_HistoryListResponse>('/api/v1/queries/history')
        .then(res => res.data?.items || []),
    delete: async (id: string): Promise<void> => { 
      await _delete<void>(`/api/v1/queries/history/${id}`); 
    },
    clear: async (): Promise<void> => { 
      await _delete<void>('/api/v1/queries/history/clear'); 
    },
  },
  favorites: {
    list: async (): Promise<FavoriteItem[]> => 
      await get<ApiResponse_FavoriteListResponse>('/api/v1/queries/favorites')
        .then(res => res.data?.items || []),
    add: async (params: FavoriteParams): Promise<FavoriteItem> =>
      await post<FavoriteItem>('/api/v1/queries/favorites', params as any),
    get: async (id: string): Promise<FavoriteItem> => 
      await get<FavoriteItem>(`/api/v1/queries/favorites/${id}`),
    update: async (id: string, params: UpdateFavoriteParams): Promise<FavoriteItem> =>
      await post<FavoriteItem>(`/api/v1/queries/favorites/${id}`, params as any),
    delete: async (id: string): Promise<void> => { 
      await _delete<void>(`/api/v1/queries/favorites/${id}`); 
    },
    clear: async (): Promise<void> => { 
      await _delete<void>('/api/v1/queries/favorites/clear'); 
    },
  },
};

export default queryHistoryService;
