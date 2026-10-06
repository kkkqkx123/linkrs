import { writable, get } from 'svelte/store';
import type { QueryResult, QueryError } from '$types/query';
import { queryService, type BatchStatementResult } from '$services/query';
import { getStreamEligibility } from '$utils/gql';
import {
	clampAutoThreshold,
	DEFAULT_AUTO_STREAM_THRESHOLD,
	resolveAutoPath,
	type ExecutionPreference,
} from '$utils/autoRoute';
import { t } from '$i18n';
import { streamStore, type StreamState } from '$stores/stream';
import { cursorStore, type CursorState } from '$stores/cursor';
import { historyStore, type QueryHistoryItem, type QueryFavoriteItem } from '$stores/history';

export type { QueryHistoryItem, QueryFavoriteItem, StreamState, CursorState };

export interface StatementResultEntry {
	id: string;
	query: string;
	success: boolean;
	result: QueryResult | null;
	error: QueryError | null;
	executionTime: number;
	truncated: boolean;
	traceId?: string;
	stages?: Record<string, number> | null;
	planNodeCount?: number | null;
}

export type ResultMode = 'materialized' | 'stream' | 'cursor' | null;

export interface AutoDecision {
	path: 'stream' | 'materialized';
	estimatedRows: number | null;
	threshold: number;
}

interface ConsoleState {
	editorContent: string;
	isExecuting: boolean;
	currentResult: QueryResult | null;
	results: StatementResultEntry[];
	executionTime: number;
	error: QueryError | null;
	activeView: 'table' | 'json' | 'graph';
	parameters: Record<string, unknown>;
	sessionVariables: Record<string, unknown>;
	resultMode: ResultMode;
	executionPreference: ExecutionPreference;
	autoStreamThreshold: number;
	autoDecision: AutoDecision | null;
}

function generateId(): string {
	return `${Date.now()}-${Math.random().toString(36).substr(2, 9)}`;
}

function loadPersisted(): Partial<ConsoleState> {
	try {
		const saved = localStorage.getItem('graphdb-console-storage');
		if (saved) return JSON.parse(saved);
	} catch {
		/* ignore */
	}
	return {};
}

function persist(state: ConsoleState): void {
	try {
		localStorage.setItem('graphdb-console-storage', JSON.stringify({
			activeView: state.activeView,
			executionPreference: state.executionPreference,
			autoStreamThreshold: state.autoStreamThreshold,
		}));
	} catch {
		/* ignore */
	}
}

const persisted = loadPersisted();

function restorePreference(): ExecutionPreference {
	const raw = persisted.executionPreference;
	if (raw === 'materialized' || raw === 'stream' || raw === 'auto') return raw;
	return 'materialized';
}

function toEntry(item: BatchStatementResult): StatementResultEntry {
	return {
		id: generateId(),
		query: item.query,
		success: item.success,
		result: item.data ?? null,
		error: item.error ?? null,
		executionTime: item.executionTime ?? 0,
		truncated: item.truncated === true,
		traceId: item.traceId,
		stages: (item.stages ?? null) as Record<string, number> | null,
		planNodeCount: item.planNodeCount ?? null,
	};
}

function loadBindings(key: string): Record<string, unknown> {
	try {
		const saved = localStorage.getItem(key);
		if (!saved) return {};
		const parsed: unknown = JSON.parse(saved);
		if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
			return parsed as Record<string, unknown>;
		}
	} catch {
		/* ignore */
	}
	return {};
}

export function createConsoleStore() {
	const { subscribe, update } = writable<ConsoleState>({
		editorContent: localStorage.getItem('graphdb_editor_draft') || '',
		isExecuting: false,
		currentResult: null,
		results: [],
		executionTime: 0,
		error: null,
		activeView: (persisted.activeView as 'table' | 'json' | 'graph') || 'table',
		parameters: loadBindings('graphdb_console_parameters'),
		sessionVariables: loadBindings('graphdb_console_session_variables'),
		resultMode: null,
		executionPreference: restorePreference(),
		autoStreamThreshold: clampAutoThreshold(
			typeof persisted.autoStreamThreshold === 'number'
				? persisted.autoStreamThreshold
				: DEFAULT_AUTO_STREAM_THRESHOLD,
		),
		autoDecision: null,
	});

	function commit(fn: (s: ConsoleState) => ConsoleState) {
		update(fn);
		persist(get({ subscribe }));
	}

	async function runStatements(rawScript: string) {
		if (!rawScript.trim()) {
			update((s) => ({
				...s,
				error: { code: 'EMPTY_QUERY', message: t('errors.queryEmpty') },
			}));
			return;
		}
		streamStore.abortActive();
		await cursorStore.close();
		const bindings = {
			parameters: get({ subscribe }).parameters,
			sessionVariables: get({ subscribe }).sessionVariables,
		};
		update((s) => ({
			...s,
			isExecuting: true,
			error: null,
			currentResult: null,
			results: [],
			resultMode: 'materialized',
		}));
		try {
			const response = await queryService.executeBatch(rawScript, bindings);
			if (response.results.length === 0) {
				update((s) => ({
					...s,
					isExecuting: false,
					error: { code: 'EMPTY_QUERY', message: t('errors.noValidQueries') },
				}));
				return;
			}
			const entries = response.results.map(toEntry);
			const primary = entries.find((e) => e.success) ?? entries[0] ?? null;
			update((s) => ({
				...s,
				isExecuting: false,
				results: entries,
				currentResult: primary?.result ?? null,
				executionTime: response.totalExecutionTime,
				error:
					entries.length > 0 && entries.every((e) => !e.success)
						? (entries[0].error ?? {
								code: 'EXECUTION_ERROR',
								message: t('errors.queryFailed'),
							})
						: null,
			}));
			for (const entry of entries) {
				historyStore.addHistory({
					query: entry.query,
					executionTime: entry.executionTime,
					rowCount: entry.result?.rowCount ?? 0,
					success: entry.success && !entry.truncated,
					path: 'materialized',
					errorCode: entry.success
						? entry.truncated
							? 'ROW_LIMIT_EXCEEDED'
							: undefined
						: entry.error?.code,
					traceId: entry.traceId,
				});
			}
		} catch (error) {
			update((s) => ({
				...s,
				isExecuting: false,
				error: {
					code: 'EXECUTION_ERROR',
					message: error instanceof Error ? error.message : t('errors.executeQuery'),
				},
			}));
		}
	}

	async function runStream(rawText: string) {
		const state = get({ subscribe });
		const bindings = {
			parameters: state.parameters,
			sessionVariables: state.sessionVariables,
		};
		update((s) => ({ ...s, autoDecision: null, resultMode: 'stream' }));
		const outcome = await streamStore.run(rawText, bindings);
		if (outcome.cancelled || !outcome.doneReceived) {
			recordStreamHistory(false);
			return;
		}
		if (outcome.streamError) {
			recordStreamHistory(false);
			return;
		}
		recordStreamHistory(true);
	}

	function recordStreamHistory(doneOk: boolean) {
		const snapshot = get(streamStore);
		if (!snapshot || snapshot.cards.length === 0) return;
		const overallCode = snapshot.error?.code ?? '';
		const cancelled =
			overallCode === 'STREAM_CANCELLED' ||
			overallCode === 'STREAM_INTERRUPTED';
		const ordered = [...snapshot.cards].sort((a, b) => a.index - b.index);
		for (const card of ordered) {
			if (card.status === 'pending') continue;
			const success = doneOk && card.status === 'completed';
			const streamStatus =
				card.status === 'completed'
					? 'completed'
					: cancelled
						? 'cancelled'
						: 'failed';
			historyStore.addHistory({
				query: card.query,
				executionTime: card.executionTime || snapshot.executionTime,
				rowCount: card.receivedCount,
				success,
				path: 'stream',
				receivedCount: card.receivedCount,
				reportedTotal: card.reportedTotal,
				streamStatus,
				errorCode: success
					? undefined
					: (card.error?.code ?? snapshot.error?.code),
			});
		}
	}

	async function executeRouted(rawText: string) {
		const snapshot = get({ subscribe });
		const preference: ExecutionPreference = snapshot.executionPreference;
		const threshold = snapshot.autoStreamThreshold;
		update((s) => ({ ...s, autoDecision: null }));

		if (preference === 'stream') {
			await runStream(rawText);
			return;
		}
		if (preference !== 'auto') {
			await runStatements(rawText);
			return;
		}

		const eligibility = getStreamEligibility(rawText);
		if (eligibility.mode === null) {
			await runStatements(rawText);
			return;
		}
		if (eligibility.mode === 'batch') {
			update((s) => ({
				...s,
				autoDecision: { path: 'stream', estimatedRows: null, threshold },
			}));
			await runStream(rawText);
			return;
		}

		const outcome = await queryService.validate(
			eligibility.statement,
			undefined,
			true,
		);
		if (!outcome.valid) {
			update((s) => ({
				...s,
				autoDecision: { path: 'materialized', estimatedRows: null, threshold },
			}));
			await runStatements(rawText);
			return;
		}
		const path = resolveAutoPath(
			{ mode: 'single', estimatedRows: outcome.estimatedRows },
			threshold,
		);
		update((s) => ({
			...s,
			autoDecision: { path, estimatedRows: outcome.estimatedRows, threshold },
		}));
		if (path === 'stream') {
			await runStream(rawText);
		} else {
			await runStatements(rawText);
		}
	}

	return {
		subscribe,
		setEditorContent: (content: string) => {
			update((s) => ({ ...s, editorContent: content }));
			localStorage.setItem('graphdb_editor_draft', content);
		},
		setParameters: (parameters: Record<string, unknown>) => {
			update((s) => ({ ...s, parameters }));
			try {
				localStorage.setItem('graphdb_console_parameters', JSON.stringify(parameters));
			} catch {
				/* ignore */
			}
		},
		setSessionVariables: (sessionVariables: Record<string, unknown>) => {
			update((s) => ({ ...s, sessionVariables }));
			try {
				localStorage.setItem('graphdb_console_session_variables', JSON.stringify(sessionVariables));
			} catch {
				/* ignore */
			}
		},
		executeQuery: async () => {
			const state = get({ subscribe });
			await executeRouted(state.editorContent);
		},
		executeQueryByText: async (query: string) => {
			await executeRouted(query);
		},
		startStream: async (query: string) => {
			await runStream(query);
		},
		openCursor: async (query: string) => {
			const eligibility = getStreamEligibility(query);
			if (!eligibility.eligible || eligibility.mode !== 'single') {
				await runStatements(query);
				return;
			}
			streamStore.abortActive();
			cursorStore.reset();
			update((s) => ({ ...s, resultMode: 'cursor', autoDecision: null }));
			await cursorStore.open(query);
		},
		fetchMoreCursor: async () => {
			await cursorStore.fetchMore();
		},
		closeCursor: async () => {
			await cursorStore.close();
			update((s) => ({ ...s, resultMode: null }));
		},
		cancelStream: () => {
			streamStore.cancel();
		},
		clearResult: () => {
			streamStore.reset();
			cursorStore.reset();
			update((s) => ({
				...s,
				currentResult: null,
				results: [],
				executionTime: 0,
				error: null,
				resultMode: null,
			}));
		},
		setActiveView: (view: 'table' | 'json' | 'graph') =>
			commit((s) => ({ ...s, activeView: view })),
		setExecutionPreference: (preference: ExecutionPreference) =>
			commit((s) => ({
				...s,
				executionPreference: preference,
			})),
		setAutoStreamThreshold: (threshold: number) =>
			commit((s) => ({
				...s,
				autoStreamThreshold: clampAutoThreshold(threshold),
			})),
		addToHistory: (item: Omit<QueryHistoryItem, 'id' | 'timestamp'>) =>
			historyStore.addHistory(item),
		clearHistory: () => historyStore.clearHistory(),
		loadFromHistory: (query: string) =>
			update((s) => ({ ...s, editorContent: query })),
		addToFavorites: (name: string, query: string) =>
			historyStore.addFavorite(name, query, get({ subscribe }).executionPreference),
		removeFromFavorites: (id: string) =>
			historyStore.removeFavorite(id),
		loadFromFavorites: (query: string) =>
			update((s) => ({ ...s, editorContent: query })),
		isFavoriteNameExists: (name: string): boolean =>
			historyStore.isFavoriteNameExists(name),
	};
}

export const consoleStore = createConsoleStore();
