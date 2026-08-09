import { writable } from 'svelte/store';
import type { QueryResult, QueryError } from '$types/query';
import { splitQueries } from '$utils/gql';
import { queryService } from '$services/query';

export interface QueryHistoryItem {
  id: string;
  query: string;
  executionTime: number;
  timestamp: number;
  rowCount: number;
  success: boolean;
}

export interface QueryFavoriteItem {
  id: string;
  name: string;
  query: string;
  createdAt: number;
}

interface ConsoleState {
  editorContent: string;
  isExecuting: boolean;
  currentResult: QueryResult | null;
  executionTime: number;
  error: QueryError | null;
  activeView: 'table' | 'json' | 'graph';
  history: QueryHistoryItem[];
  favorites: QueryFavoriteItem[];
}

const generateId = () => `${Date.now()}-${Math.random().toString(36).substr(2, 9)}`;

function loadPersisted(): Partial<ConsoleState> {
  try {
    const saved = localStorage.getItem('graphdb-console-storage');
    if (saved) return JSON.parse(saved);
  } catch { /* ignore */ }
  return {};
}

function persist(state: ConsoleState) {
  localStorage.setItem('graphdb-console-storage', JSON.stringify({
    history: state.history,
    favorites: state.favorites,
    activeView: state.activeView,
  }));
}

const persisted = loadPersisted();

function createConsoleStore() {
  const { subscribe, set, update } = writable<ConsoleState>({
    editorContent: localStorage.getItem('graphdb_editor_draft') || '',
    isExecuting: false,
    currentResult: null,
    executionTime: 0,
    error: null,
    activeView: (persisted.activeView as 'table' | 'json' | 'graph') || 'table',
    history: persisted.history || [],
    favorites: persisted.favorites || [],
  });

  return {
    subscribe,
    setEditorContent: (content: string) => {
      update(s => ({ ...s, editorContent: content }));
      localStorage.setItem('graphdb_editor_draft', content);
    },
    executeQuery: async () => {
      let state: ConsoleState = null!;
      update(s => { state = s; return s; });
      if (!state.editorContent.trim()) {
        update(s => ({ ...s, error: { code: 'EMPTY_QUERY', message: 'Query is empty' } }));
        return;
      }
      update(s => ({ ...s, isExecuting: true, error: null, currentResult: null }));
      try {
        const queries = splitQueries(state.editorContent);
        if (queries.length === 0) {
          update(s => ({ ...s, isExecuting: false, error: { code: 'EMPTY_QUERY', message: 'No valid queries found' } }));
          return;
        }
        const query = queries[0];
        const response = await queryService.execute({ query });
        if (response.success && response.data) {
          update(s => ({ ...s, currentResult: response.data, executionTime: response.executionTime || 0, isExecuting: false }));
          addToHistory({ query, executionTime: response.executionTime || 0, rowCount: response.data.rowCount || 0, success: true });
        } else {
          update(s => ({ ...s, error: response.error || { code: 'UNKNOWN_ERROR', message: 'Unknown error' }, executionTime: response.executionTime || 0, isExecuting: false }));
          addToHistory({ query, executionTime: response.executionTime || 0, rowCount: 0, success: false });
        }
      } catch (error) {
        update(s => ({ ...s, error: { code: 'EXECUTION_ERROR', message: error instanceof Error ? error.message : 'Failed to execute query' }, isExecuting: false }));
      }
    },
    executeQueryByText: async (query: string) => {
      if (!query.trim()) {
        update(s => ({ ...s, error: { code: 'EMPTY_QUERY', message: 'Query is empty' } }));
        return;
      }
      update(s => ({ ...s, isExecuting: true, error: null, currentResult: null, editorContent: query }));
      try {
        const response = await queryService.execute({ query });
        if (response.success && response.data) {
          update(s => ({ ...s, currentResult: response.data, executionTime: response.executionTime || 0, isExecuting: false }));
          addToHistory({ query, executionTime: response.executionTime || 0, rowCount: response.data.rowCount || 0, success: true });
        } else {
          update(s => ({ ...s, error: response.error || { code: 'UNKNOWN_ERROR', message: 'Unknown error' }, executionTime: response.executionTime || 0, isExecuting: false }));
          addToHistory({ query, executionTime: response.executionTime || 0, rowCount: 0, success: false });
        }
      } catch (error) {
        update(s => ({ ...s, error: { code: 'EXECUTION_ERROR', message: error instanceof Error ? error.message : 'Failed to execute query' }, isExecuting: false }));
      }
    },
    clearResult: () => update(s => ({ ...s, currentResult: null, executionTime: 0, error: null })),
    setActiveView: (view: 'table' | 'json' | 'graph') => update(s => ({ ...s, activeView: view })),
    addToHistory: (item: Omit<QueryHistoryItem, 'id' | 'timestamp'>) => addToHistory(item),
    clearHistory: () => update(s => ({ ...s, history: [] })),
    loadFromHistory: (query: string) => update(s => ({ ...s, editorContent: query })),
    addToFavorites: (name: string, query: string): { success: boolean; error?: string } => {
      let result = { success: false, error: '' };
      update(s => {
        if (!name.trim()) { result = { success: false, error: 'Name is required' }; return s; }
        if (!query.trim()) { result = { success: false, error: 'Query is required' }; return s; }
        if (s.favorites.some(f => f.name.toLowerCase() === name.toLowerCase())) { result = { success: false, error: 'A favorite with this name already exists' }; return s; }
        if (s.favorites.length >= 30) { result = { success: false, error: 'Maximum 30 favorites allowed' }; return s; }
        const newFav: QueryFavoriteItem = { id: generateId(), name: name.trim(), query: query.trim(), createdAt: Date.now() };
        result = { success: true };
        return { ...s, favorites: [...s.favorites, newFav] };
      });
      return result;
    },
    removeFromFavorites: (id: string) => update(s => ({ ...s, favorites: s.favorites.filter(f => f.id !== id) })),
    loadFromFavorites: (query: string) => update(s => ({ ...s, editorContent: query })),
    isFavoriteNameExists: (name: string): boolean => {
      let exists = false;
      update(s => { exists = s.favorites.some(f => f.name.toLowerCase() === name.toLowerCase()); return s; });
      return exists;
    },
  };

  function addToHistory(item: Omit<QueryHistoryItem, 'id' | 'timestamp'>) {
    update(s => {
      const newItem: QueryHistoryItem = { ...item, id: generateId(), timestamp: Date.now() };
      const newHistory = [newItem, ...s.history].slice(0, 50);
      return { ...s, history: newHistory };
    });
  }
}

export const consoleStore = createConsoleStore();