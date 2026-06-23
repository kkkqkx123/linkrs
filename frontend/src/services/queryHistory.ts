import { post, get, _delete } from '@/utils/http';
import type { HistoryItem, HistoryParams, FavoriteItem, FavoriteParams, UpdateFavoriteParams } from '@/types/query';
import type { PaginatedResponse } from '@/types/api';

export const queryHistoryService = {
  history: {
    add: async (params: HistoryParams): Promise<HistoryItem> => {
      const response = await post('/api/history')(params) as HistoryItem;
      return response;
    },

    list: async (limit?: number, offset?: number): Promise<PaginatedResponse<HistoryItem>> => {
      const response = await get('/api/history')({ limit, offset }) as PaginatedResponse<HistoryItem>;
      return response;
    },

    delete: async (id: string): Promise<void> => {
      await _delete(`/api/history/${id}`)();
    },

    clear: async (): Promise<void> => {
      await _delete('/api/history/clear')();
    },
  },

  favorites: {
    list: async (): Promise<FavoriteItem[]> => {
      const response = await get('/api/favorites')() as FavoriteItem[];
      return response;
    },

    add: async (params: FavoriteParams): Promise<FavoriteItem> => {
      const response = await post('/api/favorites')(params) as FavoriteItem;
      return response;
    },

    get: async (id: string): Promise<FavoriteItem> => {
      const response = await get(`/api/favorites/${id}`)() as FavoriteItem;
      return response;
    },

    update: async (id: string, params: UpdateFavoriteParams): Promise<FavoriteItem> => {
      const response = await post(`/api/favorites/${id}`)(params) as FavoriteItem;
      return response;
    },

    delete: async (id: string): Promise<void> => {
      await _delete(`/api/favorites/${id}`)();
    },

    clear: async (): Promise<void> => {
      await _delete('/api/favorites/clear')();
    },
  },
};

export default queryHistoryService;
