import { writable } from 'svelte/store';
import type { QueryResult, QueryError } from '$types/query';
import { splitQueries } from '$utils/gql';
import { queryService, type BatchStatementResult } from '$services/query';

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

/** One statement's outcome as rendered in the console result list. */
export interface StatementResultEntry {
  id: string;
  query: string;
  success: boolean;
  result: QueryResult | null;
  error: QueryError | null;
  executionTime: number;
}

interface ConsoleState {
  editorContent: string;
  isExecuting: boolean;
  currentResult: QueryResult | null;
  results: StatementResultEntry[];
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

/** Map one batch statement outcome into a renderable result entry. */
function toEntry(item: BatchStatementResult): StatementResultEntry {
  return {
    id: generateId(),
    query: item.query,
    success: item.success,
    result: item.data ?? null,
    error: item.error ?? null,
    executionTime: item.executionTime ?? 0,
  };
}

function createConsoleStore() {
  const { subscribe, set, update } = writable<ConsoleState>({
    editorContent: localStorage.getItem('graphdb_editor_draft') || '',
    isExecuting: false,
    currentResult: null,
    results: [],
    executionTime: 0,
    error: null,
    activeView: (persisted.activeView as 'table' | 'json' | 'graph') || 'table',
    history: persisted.history || [],
    favorites: persisted.favorites || [],
  });

  /**
   * Run every statement contained in the editor. State is cleared up front so
   * stale results never mix with a new run, then each statement is recorded
   * into both the result list and the query history.
   */
  async function runStatements(rawScript: string, echoIntoEditor: boolean) {
    if (!rawScript.trim()) {
      update(s => ({ ...s, error: { code: 'EMPTY_QUERY', message: 'Query is empty' } }));
      return;
    }
    const statements = splitQueries(rawScript);
    if (statements.length === 0) {
      update(s => ({ ...s, error: { code: 'EMPTY_QUERY', message: 'No valid queries found' } }));
      return;
    }
    update(s => ({
      ...s,
      isExecuting: true,
      error: null,
      currentResult: null,
      results: [],
      ...(echoIntoEditor ? { editorContent: rawScript } : {}),
    }));
    try {
      const response = await queryService.executeBatch(rawScript);
      const entries = response.results.map(toEntry);
      const primary = entries.find(e => e.success) ?? entries[0] ?? null;
      update(s => ({
        ...s,
        isExecuting: false,
        results: entries,
        currentResult: primary?.result ?? null,
        executionTime: response.totalExecutionTime,
        error: entries.length > 0 && entries.every(e => !e.success)
          ? (entries[0].error ?? { code: 'EXECUTION_ERROR', message: 'Query failed' })
          : null,
      }));
      for (const entry of entries) {
        addToHistory({
          query: entry.query,
          executionTime: entry.executionTime,
          rowCount: entry.result?.rowCount ?? 0,
          success: entry.success,
        });
      }
    } catch (error) {
      update(s => ({
        ...s,
        isExecuting: false,
        error: { code: 'EXECUTION_ERROR', message: error instanceof Error ? error.message : 'Failed to execute query' },
      }));
    }
  }

  return {
    subscribe,
    setEditorContent: (content: string) => {
      update(s => ({ ...s, editorContent: content }));
      localStorage.setItem('graphdb_editor_draft', content);
    },
    executeQuery: async () => {
      let state: ConsoleState = null!;
      update(s => { state = s; return s; });
      await runStatements(state.editorContent, false);
    },
    executeQueryByText: async (query: string) => {
      await runStatements(query, true);
    },
    clearResult: () => update(s => ({ ...s, currentResult: null, results: [], executionTime: 0, error: null })),
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
        result = { success: true, error: '' };
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