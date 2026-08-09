import { post, get, _delete } from '$utils/http';
import type { HistoryItem, HistoryParams, FavoriteItem, FavoriteParams, UpdateFavoriteParams } from '$types/query';
import type { PaginatedResponse } from '$types/api';

export const queryHistoryService = {
  history: {
    add: async (params: HistoryParams): Promise<HistoryItem> =>
      await post('/api/history')(params) as HistoryItem,
    list: async (limit?: number, offset?: number): Promise<PaginatedResponse<HistoryItem>> =>
      await get('/api/history')({ limit, offset }) as PaginatedResponse<HistoryItem>,
    delete: async (id: string): Promise<void> => { await _delete(`/api/history/${id}`)(); },
    clear: async (): Promise<void> => { await _delete('/api/history/clear')(); },
  },
  favorites: {
    list: async (): Promise<FavoriteItem[]> => await get('/api/favorites')() as FavoriteItem[],
    add: async (params: FavoriteParams): Promise<FavoriteItem> =>
      await post('/api/favorites')(params) as FavoriteItem,
    get: async (id: string): Promise<FavoriteItem> => await get(`/api/favorites/${id}`)() as FavoriteItem,
    update: async (id: string, params: UpdateFavoriteParams): Promise<FavoriteItem> =>
      await post(`/api/favorites/${id}`)(params) as FavoriteItem,
    delete: async (id: string): Promise<void> => { await _delete(`/api/favorites/${id}`)(); },
    clear: async (): Promise<void> => { await _delete('/api/favorites/clear')(); },
  },
};

export default queryHistoryService;