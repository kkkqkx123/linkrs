import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type { QueryResult, QueryError } from '@/types/query';
import { splitQueries } from '@/utils/gql';
import { queryService } from '@/services/query';

// History item interface
export interface QueryHistoryItem {
  id: string;
  query: string;
  executionTime: number;
  timestamp: number;
  rowCount: number;
  success: boolean;
}

// Favorite item interface
export interface QueryFavoriteItem {
  id: string;
  name: string;
  query: string;
  createdAt: number;
}

// Console state interface
interface ConsoleState {
  // Editor state
  editorContent: string;
  isExecuting: boolean;
  cursorPosition: { line: number; column: number };

  // Result state
  currentResult: QueryResult | null;
  executionTime: number;
  error: QueryError | null;
  activeView: 'table' | 'json' | 'graph';

  // History state
  history: QueryHistoryItem[];

  // Favorites state
  favorites: QueryFavoriteItem[];

  // Actions
  setEditorContent: (content: string) => void;
  setCursorPosition: (line: number, column: number) => void;
  executeQuery: () => Promise<void>;
  executeQueryByText: (query: string) => Promise<void>;
  clearResult: () => void;
  setActiveView: (view: 'table' | 'json' | 'graph') => void;

  // History actions
  addToHistory: (item: Omit<QueryHistoryItem, 'id' | 'timestamp'>) => void;
  clearHistory: () => void;
  loadFromHistory: (query: string) => void;

  // Favorites actions
  addToFavorites: (name: string, query: string) => { success: boolean; error?: string };
  removeFromFavorites: (id: string) => void;
  loadFromFavorites: (query: string) => void;
  isFavoriteNameExists: (name: string) => boolean;
}

// Generate unique ID
const generateId = (): string => {
  return `${Date.now()}-${Math.random().toString(36).substr(2, 9)}`;
};

// Create console store with persistence
export const useConsoleStore = create<ConsoleState>()(
  persist(
    (set, get) => ({
      // Initial state
      editorContent: '',
      isExecuting: false,
      cursorPosition: { line: 1, column: 1 },
      currentResult: null,
      executionTime: 0,
      error: null,
      activeView: 'table',
      history: [],
      favorites: [],

      // Editor actions
      setEditorContent: (content: string) => {
        set({ editorContent: content });
        // Auto-save to localStorage for draft recovery
        localStorage.setItem('graphdb_editor_draft', content);
      },

      setCursorPosition: (line: number, column: number) => {
        set({ cursorPosition: { line, column } });
      },

      // Execute query
      executeQuery: async () => {
        const { editorContent, addToHistory } = get();
        
        if (!editorContent.trim()) {
          set({ error: { code: 'EMPTY_QUERY', message: 'Query is empty' } });
          return;
        }

        set({ isExecuting: true, error: null, currentResult: null });

        try {
          // Split multiple queries
          const queries = splitQueries(editorContent);
          
          if (queries.length === 0) {
            set({ 
              isExecuting: false, 
              error: { code: 'EMPTY_QUERY', message: 'No valid queries found' } 
            });
            return;
          }

          // Execute first query for now (can be extended to batch execution)
          const query = queries[0];
          const response = await queryService.execute({ query });

          if (response.success && response.data) {
            set({
              currentResult: response.data,
              executionTime: response.executionTime || 0,
              isExecuting: false,
            });

            // Add to history
            addToHistory({
              query,
              executionTime: response.executionTime || 0,
              rowCount: response.data.rowCount || 0,
              success: true,
            });
          } else {
            set({
              error: response.error || { code: 'UNKNOWN_ERROR', message: 'Unknown error' },
              executionTime: response.executionTime || 0,
              isExecuting: false,
            });

            // Add failed query to history
            addToHistory({
              query,
              executionTime: response.executionTime || 0,
              rowCount: 0,
              success: false,
            });
          }
        } catch (error) {
          set({
            error: {
              code: 'EXECUTION_ERROR',
              message: error instanceof Error ? error.message : 'Failed to execute query',
            },
            isExecuting: false,
          });
        }
      },

      // Execute specific query text
      executeQueryByText: async (query: string) => {
        if (!query.trim()) {
          set({ error: { code: 'EMPTY_QUERY', message: 'Query is empty' } });
          return;
        }

        set({ isExecuting: true, error: null, currentResult: null, editorContent: query });

        try {
          const response = await queryService.execute({ query });

          if (response.success && response.data) {
            set({
              currentResult: response.data,
              executionTime: response.executionTime || 0,
              isExecuting: false,
            });

            get().addToHistory({
              query,
              executionTime: response.executionTime || 0,
              rowCount: response.data.rowCount || 0,
              success: true,
            });
          } else {
            set({
              error: response.error || { code: 'UNKNOWN_ERROR', message: 'Unknown error' },
              executionTime: response.executionTime || 0,
              isExecuting: false,
            });

            get().addToHistory({
              query,
              executionTime: response.executionTime || 0,
              rowCount: 0,
              success: false,
            });
          }
        } catch (error) {
          set({
            error: {
              code: 'EXECUTION_ERROR',
              message: error instanceof Error ? error.message : 'Failed to execute query',
            },
            isExecuting: false,
          });
        }
      },

      // Clear result
      clearResult: () => {
        set({
          currentResult: null,
          executionTime: 0,
          error: null,
        });
      },

      // Set active view
      setActiveView: (view: 'table' | 'json' | 'graph') => {
        set({ activeView: view });
      },

      // History actions
      addToHistory: (item: Omit<QueryHistoryItem, 'id' | 'timestamp'>) => {
        const { history } = get();
        const newItem: QueryHistoryItem = {
          ...item,
          id: generateId(),
          timestamp: Date.now(),
        };

        // Keep only last 50 items
        const newHistory = [newItem, ...history].slice(0, 50);
        set({ history: newHistory });
      },

      clearHistory: () => {
        set({ history: [] });
      },

      loadFromHistory: (query: string) => {
        set({ editorContent: query });
      },

      // Favorites actions
      addToFavorites: (name: string, query: string): { success: boolean; error?: string } => {
        const { favorites, isFavoriteNameExists } = get();

        if (!name.trim()) {
          return { success: false, error: 'Name is required' };
        }

        if (!query.trim()) {
          return { success: false, error: 'Query is required' };
        }

        if (isFavoriteNameExists(name)) {
          return { success: false, error: 'A favorite with this name already exists' };
        }

        if (favorites.length >= 30) {
          return { success: false, error: 'Maximum 30 favorites allowed' };
        }

        const newFavorite: QueryFavoriteItem = {
          id: generateId(),
          name: name.trim(),
          query: query.trim(),
          createdAt: Date.now(),
        };

        set({ favorites: [...favorites, newFavorite] });
        return { success: true };
      },

      removeFromFavorites: (id: string) => {
        const { favorites } = get();
        set({ favorites: favorites.filter((f) => f.id !== id) });
      },

      loadFromFavorites: (query: string) => {
        set({ editorContent: query });
      },

      isFavoriteNameExists: (name: string): boolean => {
        const { favorites } = get();
        return favorites.some((f) => f.name.toLowerCase() === name.toLowerCase());
      },
    }),
    {
      name: 'graphdb-console-storage',
      partialize: (state) => ({
        history: state.history,
        favorites: state.favorites,
        activeView: state.activeView,
      }),
    }
  )
);

// Initialize editor content from draft
export const initEditorFromDraft = (): void => {
  const draft = localStorage.getItem('graphdb_editor_draft');
  if (draft) {
    useConsoleStore.setState({ editorContent: draft });
  }
};

export default useConsoleStore;
