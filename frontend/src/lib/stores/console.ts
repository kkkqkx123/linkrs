import { writable, get } from 'svelte/store';
import type { QueryResult, QueryError } from '$types/query';
import { queryService, type BatchStatementResult } from '$services/query';
import { streamQuery } from '$services/streamQuery';
import { cursorService } from '$services/cursor';
import { getStreamEligibility } from '$utils/gql';
import {
	clampAutoThreshold,
	DEFAULT_AUTO_STREAM_THRESHOLD,
	resolveAutoPath,
	type ExecutionPreference,
} from '$utils/autoRoute';
import { t } from '$i18n';

export interface QueryHistoryItem {
	id: string;
	query: string;
	executionTime: number;
	timestamp: number;
	rowCount: number;
	success: boolean;
	/** Execution path that produced this entry, when recorded. */
	path?: 'materialized' | 'stream';
	/** Rows actually received by the client (stream runs). */
	receivedCount?: number;
	/** Total rows reported by the server (stream runs). */
	reportedTotal?: number | null;
	/** Terminal stream state for stream runs. */
	streamStatus?: 'completed' | 'failed' | 'cancelled';
	/** Failure code for failed runs, when known. */
	errorCode?: string;
	/** Backend trace id linking the run to its query portrait. */
	traceId?: string;
}

export interface QueryFavoriteItem {
	id: string;
	name: string;
	query: string;
	createdAt: number;
	/** Execution preference at save time, shown as a hint only. */
	preferredPath?: 'materialized' | 'stream' | 'auto';
}

/** One statement's outcome as rendered in the console result list. */
export interface StatementResultEntry {
	id: string;
	query: string;
	success: boolean;
	result: QueryResult | null;
	error: QueryError | null;
	executionTime: number;
	truncated: boolean;
	/** Backend trace id for portrait lookup. */
	traceId?: string;
	/** Per-stage timings for the hover breakdown. */
	stages?: Record<string, number> | null;
	planNodeCount?: number | null;
}

export type StreamStatus =
	'idle' | 'connecting' | 'receiving' | 'completed' | 'failed' | 'cancelled';

export type StreamCardStatus =
	'pending' | 'receiving' | 'completed' | 'failed' | 'cancelled';

/** One statement's progressive buffer inside a stream run. */
export interface StreamCardState {
	index: number;
	query: string;
	columns: string[];
	rows: Record<string, unknown>[];
	receivedCount: number;
	/** Server-reported total, when the summary arrived (may lag receives). */
	reportedTotal: number | null;
	executionTime: number;
	status: StreamCardStatus;
	error: QueryError | null;
}

export interface StreamState {
	/** Overall connection status across all cards. */
	status: StreamStatus;
	/** Original script text that started this run. */
	query: string;
	batch: boolean;
	cards: StreamCardState[];
	executionTime: number;
	/** Fatal error outside any card (connection failure, interruption). */
	error: QueryError | null;
	startedAt: number;
	firstRowAt: number | null;
	doneReceived: boolean;
}

export type ResultMode = 'materialized' | 'stream' | 'cursor' | null;

export type CursorStatus =
	'idle' | 'opening' | 'open' | 'fetching' | 'exhausted' | 'failed';

/** Forward-only cursor run: pages append into `rows`, never the full set. */
export interface CursorState {
	status: CursorStatus;
	query: string;
	cursorId: number | null;
	columns: string[];
	rows: Record<string, unknown>[];
	receivedCount: number;
	hasMore: boolean;
	pageSize: number;
	error: QueryError | null;
	startedAt: number;
}

/** Why the latest auto-mode run took the path it did. */
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
	history: QueryHistoryItem[];
	favorites: QueryFavoriteItem[];
	parameters: Record<string, unknown>;
	sessionVariables: Record<string, unknown>;
	stream: StreamState;
	resultMode: ResultMode;
	cursor: CursorState;
	executionPreference: ExecutionPreference;
	autoStreamThreshold: number;
	autoDecision: AutoDecision | null;
}

const generateId = () =>
	`${Date.now()}-${Math.random().toString(36).substr(2, 9)}`;

function loadPersisted(): Partial<ConsoleState> {
	try {
		const saved = localStorage.getItem('graphdb-console-storage');
		if (saved) return JSON.parse(saved);
	} catch {
		/* ignore */
	}
	return {};
}

function persist(state: ConsoleState) {
	localStorage.setItem(
		'graphdb-console-storage',
		JSON.stringify({
			history: state.history,
			favorites: state.favorites,
			activeView: state.activeView,
			executionPreference: state.executionPreference,
			autoStreamThreshold: state.autoStreamThreshold,
		}),
	);
}

const persisted = loadPersisted();

function restorePreference(): ExecutionPreference {
	const raw = persisted.executionPreference;
	if (raw === 'materialized' || raw === 'stream' || raw === 'auto') return raw;
	return 'materialized';
}

/** Map one batch statement outcome into a renderable result entry. */
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

function emptyCard(index: number, query: string): StreamCardState {
	return {
		index,
		query,
		columns: [],
		rows: [],
		receivedCount: 0,
		reportedTotal: null,
		executionTime: 0,
		status: 'pending',
		error: null,
	};
}

function idleStream(): StreamState {
	return {
		status: 'idle',
		query: '',
		batch: false,
		cards: [],
		executionTime: 0,
		error: null,
		startedAt: 0,
		firstRowAt: null,
		doneReceived: false,
	};
}

/** Place a row into a card's sparse buffer, counting first arrivals. */
function placeCardRow(
	card: StreamCardState,
	row: Record<string, unknown>,
	index: number,
): void {
	const rows = card.rows;
	if (index >= rows.length) rows.length = index + 1;
	if (rows[index] === undefined) {
		rows[index] = row;
		card.receivedCount += 1;
	} else {
		rows[index] = row;
	}
}

/** Rows per cursor fetch; mirrors the server default page size. */
const CURSOR_PAGE_SIZE = 500;

function idleCursor(): CursorState {
	return {
		status: 'idle',
		query: '',
		cursorId: null,
		columns: [],
		rows: [],
		receivedCount: 0,
		hasMore: false,
		pageSize: CURSOR_PAGE_SIZE,
		error: null,
		startedAt: 0,
	};
}

let streamController: AbortController | null = null;
let streamGeneration = 0;

function abortActiveStream() {
	streamController?.abort();
	streamController = null;
}

function createConsoleStore() {
	const { subscribe, update } = writable<ConsoleState>({
		editorContent: localStorage.getItem('graphdb_editor_draft') || '',
		isExecuting: false,
		currentResult: null,
		results: [],
		executionTime: 0,
		error: null,
		activeView: (persisted.activeView as 'table' | 'json' | 'graph') || 'table',
		history: persisted.history || [],
		favorites: persisted.favorites || [],
		parameters: loadBindings('graphdb_console_parameters'),
		sessionVariables: loadBindings('graphdb_console_session_variables'),
		stream: idleStream(),
		resultMode: null,
		cursor: idleCursor(),
		executionPreference: restorePreference(),
		autoStreamThreshold: clampAutoThreshold(
			typeof persisted.autoStreamThreshold === 'number'
				? persisted.autoStreamThreshold
				: DEFAULT_AUTO_STREAM_THRESHOLD,
		),
		autoDecision: null,
	});

	/** Mutate state and write the persisted slice back to localStorage. */
	function commit(fn: (s: ConsoleState) => ConsoleState) {
		update(fn);
		persist(get({ subscribe }));
	}

	/**
	 * Run every statement contained in the editor. State is cleared up front so
	 * stale results never mix with a new run, then each statement is recorded
	 * into both the result list and the query history. The editor draft is left
	 * untouched so running a selection never destroys the full script.
	 */
	async function runStatements(rawScript: string) {
		if (!rawScript.trim()) {
			update((s) => ({
				...s,
				error: { code: 'EMPTY_QUERY', message: t('errors.queryEmpty') },
			}));
			return;
		}
		abortActiveStream();
		await closeCursorSilent();
		streamGeneration += 1;
		let bindings: {
			parameters: Record<string, unknown>;
			sessionVariables: Record<string, unknown>;
		} = { parameters: {}, sessionVariables: {} };
		update((s) => {
			bindings = {
				parameters: s.parameters,
				sessionVariables: s.sessionVariables,
			};
			return {
				...s,
				isExecuting: true,
				error: null,
				currentResult: null,
				results: [],
				stream: idleStream(),
				resultMode: 'materialized',
			};
		});
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
				addToHistory({
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
					message:
						error instanceof Error ? error.message : t('errors.executeQuery'),
				},
			}));
		}
	}

	/**
	 * Run text through the streaming endpoint, appending rows as they arrive.
	 * A single plain statement streams alone; a multi-statement script streams
	 * as a batch with one card per statement. Anything else falls back to the
	 * materialized batch path.
	 */
	async function startStream(rawText: string) {
		update((s) => ({ ...s, autoDecision: null }));
		await runStream(rawText);
	}

	/** Streaming body shared by the explicit entry and auto routing. */
	async function runStream(rawText: string) {
		await closeCursorSilent();
		const eligibility = getStreamEligibility(rawText);
		if (!eligibility.eligible || eligibility.mode === null) {
			await runStatements(rawText);
			return;
		}
		const batch = eligibility.mode === 'batch';
		const cardQueries = batch
			? eligibility.statements
			: [eligibility.statement];
		let bindings: {
			parameters: Record<string, unknown>;
			sessionVariables: Record<string, unknown>;
		} = { parameters: {}, sessionVariables: {} };
		abortActiveStream();
		streamGeneration += 1;
		const generation = streamGeneration;
		const controller = new AbortController();
		streamController = controller;
		const startedAt = Date.now();
		update((s) => {
			bindings = {
				parameters: s.parameters,
				sessionVariables: s.sessionVariables,
			};
			return {
				...s,
				resultMode: 'stream',
				stream: {
					...idleStream(),
					status: 'connecting',
					query: rawText,
					batch,
					cards: cardQueries.map((query, index) => emptyCard(index, query)),
					startedAt,
				},
			};
		});
		const isCurrent = () => generation === streamGeneration;
		const cardQuery = batch
			? {
					query: '',
					statements: cardQueries,
					parameters: bindings.parameters,
					sessionVariables: bindings.sessionVariables,
					failFast: true,
				}
			: { query: cardQueries[0] };
		try {
			const outcome = await streamQuery(
				{ ...cardQuery, signal: controller.signal },
				{
					onStatementBegin: ({ index }) => {
						if (!isCurrent()) return;
						update((s) => {
							const cards = s.stream.cards.map((card) =>
								card.index === index && card.status === 'pending'
									? { ...card, status: 'receiving' as StreamCardStatus }
									: card,
							);
							return { ...s, stream: { ...s.stream, cards } };
						});
					},
					onStatementEnd: (info) => {
						if (!isCurrent()) return;
						update((s) => {
							const cards = s.stream.cards.map((card) =>
								card.index === info.index
									? {
											...card,
											status: info.success
												? ('completed' as StreamCardStatus)
												: ('failed' as StreamCardStatus),
											reportedTotal: info.rowsReturned,
											executionTime: card.executionTime || info.executionTimeMs,
											error: info.success
												? card.error
												: (card.error ?? {
														code: info.code ?? 'QUERY_ERROR',
														message:
															info.message ?? t('errors.statementFailed'),
													}),
										}
									: card,
							);
							return { ...s, stream: { ...s.stream, cards } };
						});
					},
					onSchema: (columns, stmt) => {
						if (!isCurrent()) return;
						const target = stmt;
						update((s) => ({
							...s,
							stream: {
								...s.stream,
								cards: s.stream.cards.map((card) =>
									card.index === target ? { ...card, columns } : card,
								),
							},
						}));
					},
					onRow: (row, index, stmt) => {
						if (!isCurrent()) return;
						const target = stmt;
						update((s) => {
							const cards = s.stream.cards.map((card) => {
								if (card.index !== target) return card;
								const next: StreamCardState = {
									...card,
									rows: [...card.rows],
									status: 'receiving' as StreamCardStatus,
								};
								placeCardRow(next, row, index);
								return next;
							});
							return {
								...s,
								stream: {
									...s.stream,
									status: 'receiving',
									cards,
									firstRowAt: s.stream.firstRowAt ?? Date.now(),
								},
							};
						});
					},
					onMetadata: (info) => {
						if (!isCurrent()) return;
						const target = info.stmt;
						update((s) => ({
							...s,
							stream: {
								...s.stream,
								cards: s.stream.cards.map((card) =>
									card.index === target
										? {
												...card,
												executionTime: info.executionTimeMs,
												reportedTotal: info.rowsReturned,
											}
										: card,
								),
							},
						}));
					},
					onStreamError: (streamError) => {
						if (!isCurrent()) return;
						const target = streamError.stmt;
						update((s) => ({
							...s,
							stream: {
								...s.stream,
								cards: s.stream.cards.map((card) =>
									card.index === target
										? {
												...card,
												error: {
													code: streamError.code,
													message: streamError.message,
												},
											}
										: card,
								),
							},
						}));
					},
				},
			);
			if (!isCurrent()) return;
			if (streamController === controller) streamController = null;
			if (outcome.cancelled || controller.signal.aborted) {
				failOverall(
					'STREAM_CANCELLED',
					t('errors.streamCancelled'),
					startedAt,
					true,
				);
				recordCardsHistory(false);
				return;
			}
			if (!outcome.doneReceived) {
				failOverall(
					'STREAM_INTERRUPTED',
					t('errors.streamInterrupted'),
					startedAt,
					true,
				);
				recordCardsHistory(false);
				return;
			}
			if (outcome.streamError && !batch) {
				update((s) => ({
					...s,
					stream: {
						...s.stream,
						status: 'failed',
						doneReceived: true,
						executionTime:
							s.stream.executionTime ||
							outcome.executionTimeMs ||
							Date.now() - startedAt,
					},
				}));
				recordCardsHistory(false);
				return;
			}
			update((s) => ({
				...s,
				stream: {
					...s.stream,
					status: 'completed',
					doneReceived: true,
					executionTime: s.stream.executionTime || Date.now() - startedAt,
				},
			}));
			recordCardsHistory(true);
		} catch (error) {
			if (!isCurrent()) return;
			if (streamController === controller) streamController = null;
			if (controller.signal.aborted) {
				failOverall(
					'STREAM_CANCELLED',
					t('errors.streamCancelled'),
					startedAt,
					true,
				);
				recordCardsHistory(false);
				return;
			}
			failOverall(
				'STREAM_CONNECTION_ERROR',
				error instanceof Error ? error.message : t('errors.openStream'),
				startedAt,
			);
			recordCardsHistory(false);
		}
	}

	/** Mark the run ended overall and settle every card still in flight. */
	function failOverall(
		code: string,
		message: string,
		startedAt: number,
		cancelled = false,
	) {
		const status = cancelled ? 'cancelled' : 'failed';
		update((s) => ({
			...s,
			stream: {
				...s.stream,
				status,
				executionTime: s.stream.executionTime || Date.now() - startedAt,
				error: s.stream.error ?? { code, message },
				cards: s.stream.cards.map((card) =>
					card.status === 'receiving' ||
					(card.status === 'pending' && !s.stream.batch)
						? {
								...card,
								status: status as StreamCardStatus,
								error: card.error ?? { code, message },
							}
						: card,
				),
			},
		}));
	}

	/** One history line per statement that produced output, in batch order. */
	function recordCardsHistory(doneOk: boolean) {
		const snapshot = get({ subscribe }).stream;
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
			addToHistory({
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

	/**
	 * Route a run by execution preference. Explicit modes clear the auto
	 * decision strip; auto mode validates a single statement, compares the
	 * planner estimate against the threshold, and records why it chose
	 * the path. Batch scripts skip validation and stream directly.
	 */
	async function executeRouted(rawText: string) {
		const snapshot = get({ subscribe });
		const preference: ExecutionPreference = snapshot.executionPreference;
		const threshold = snapshot.autoStreamThreshold;
		update((s) => ({ ...s, autoDecision: null }));
		if (preference === 'stream') {
			await startStream(rawText);
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

	let cursorGeneration = 0;

	/** Release the open cursor without touching the result mode. */
	async function closeCursorSilent() {
		let cursorId: number | null = null;
		update((s) => {
			cursorId = s.cursor.cursorId;
			return s;
		});
		if (cursorId !== null) {
			await cursorService.close(cursorId);
		}
		update((s) => ({ ...s, cursor: idleCursor() }));
	}

	/**
	 * Page through a single statement with a server cursor instead of
	 * buffering the full result. Only single plain statements qualify;
	 * anything else falls back to the materialized batch path.
	 */
	async function openCursor(rawText: string) {
		const eligibility = getStreamEligibility(rawText);
		if (!eligibility.eligible || eligibility.mode !== 'single') {
			await runStatements(rawText);
			return;
		}
		abortActiveStream();
		await closeCursorSilent();
		cursorGeneration += 1;
		const generation = cursorGeneration;
		const statement = eligibility.statement;
		const startedAt = Date.now();
		update((s) => ({
			...s,
			resultMode: 'cursor',
			autoDecision: null,
			cursor: {
				...idleCursor(),
				status: 'opening',
				query: statement,
				startedAt,
			},
		}));
		try {
			const opened = await cursorService.open(statement);
			if (generation !== cursorGeneration) {
				await cursorService.close(opened.cursorId);
				return;
			}
			update((s) => ({
				...s,
				cursor: {
					...s.cursor,
					status: 'open',
					cursorId: opened.cursorId,
					columns: opened.columns,
				},
			}));
			await fetchMoreCursor();
		} catch (error) {
			if (generation !== cursorGeneration) return;
			update((s) => ({
				...s,
				cursor: {
					...s.cursor,
					status: 'failed',
					error: {
						code: 'CURSOR_OPEN_FAILED',
						message:
							error instanceof Error ? error.message : t('errors.openCursor'),
					},
				},
			}));
		}
	}

	/** Fetch the next page into the cursor buffer. */
	async function fetchMoreCursor() {
		let snapshot: CursorState = null!;
		update((s) => {
			snapshot = s.cursor;
			return s;
		});
		if (snapshot.status !== 'open' || snapshot.cursorId === null) return;
		// The first fetch runs before the server reports exhaustion.
		if (!snapshot.hasMore && snapshot.receivedCount > 0) return;
		const generation = cursorGeneration;
		const cursorId = snapshot.cursorId;
		const pageSize = snapshot.pageSize;
		update((s) => ({ ...s, cursor: { ...s.cursor, status: 'fetching' } }));
		try {
			const page = await cursorService.fetch(cursorId, pageSize);
			if (generation !== cursorGeneration) return;
			update((s) => {
				const rows = [...s.cursor.rows, ...page.rows];
				return {
					...s,
					cursor: {
						...s.cursor,
						status: page.hasMore ? 'open' : 'exhausted',
						columns: page.columns.length > 0 ? page.columns : s.cursor.columns,
						rows,
						receivedCount: rows.length,
						hasMore: page.hasMore,
						error: null,
					},
				};
			});
		} catch (error) {
			if (generation !== cursorGeneration) return;
			update((s) => ({
				...s,
				cursor: {
					...s.cursor,
					status: 'failed',
					error: {
						code: 'CURSOR_FETCH_FAILED',
						message:
							error instanceof Error
								? error.message
								: t('errors.fetchCursorPage'),
					},
				},
			}));
		}
	}

	/** Close the cursor and clear its view. */
	async function closeCursor() {
		cursorGeneration += 1;
		await closeCursorSilent();
		update((s) => ({ ...s, resultMode: null }));
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
				localStorage.setItem(
					'graphdb_console_parameters',
					JSON.stringify(parameters),
				);
			} catch {
				/* ignore */
			}
		},
		setSessionVariables: (sessionVariables: Record<string, unknown>) => {
			update((s) => ({ ...s, sessionVariables }));
			try {
				localStorage.setItem(
					'graphdb_console_session_variables',
					JSON.stringify(sessionVariables),
				);
			} catch {
				/* ignore */
			}
		},
		executeQuery: async () => {
			let state: ConsoleState = null!;
			update((s) => {
				state = s;
				return s;
			});
			await executeRouted(state.editorContent);
		},
		executeQueryByText: async (query: string) => {
			await executeRouted(query);
		},
		startStream: async (query: string) => {
			await startStream(query);
		},
		openCursor: async (query: string) => {
			await openCursor(query);
		},
		fetchMoreCursor: async () => {
			await fetchMoreCursor();
		},
		closeCursor: async () => {
			await closeCursor();
		},
		cancelStream: () => {
			abortActiveStream();
		},
		clearResult: () => {
			abortActiveStream();
			streamGeneration += 1;
			cursorGeneration += 1;
			let cursorId: number | null = null;
			update((s) => {
				cursorId = s.cursor.cursorId;
				return {
					...s,
					currentResult: null,
					results: [],
					executionTime: 0,
					error: null,
					stream: idleStream(),
					cursor: idleCursor(),
					resultMode: null,
				};
			});
			if (cursorId !== null) void cursorService.close(cursorId);
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
			addToHistory(item),
		clearHistory: () => commit((s) => ({ ...s, history: [] })),
		loadFromHistory: (query: string) =>
			update((s) => ({ ...s, editorContent: query })),
		addToFavorites: (
			name: string,
			query: string,
		): { success: boolean; error?: string } => {
			let result = { success: false, error: '' };
			commit((s) => {
				if (!name.trim()) {
					result = { success: false, error: t('errors.favoriteNameRequired') };
					return s;
				}
				if (!query.trim()) {
					result = { success: false, error: t('errors.favoriteQueryRequired') };
					return s;
				}
				if (
					s.favorites.some((f) => f.name.toLowerCase() === name.toLowerCase())
				) {
					result = { success: false, error: t('errors.favoriteDuplicateName') };
					return s;
				}
				if (s.favorites.length >= 30) {
					result = { success: false, error: t('errors.favoriteLimitReached') };
					return s;
				}
				const newFav: QueryFavoriteItem = {
					id: generateId(),
					name: name.trim(),
					query: query.trim(),
					createdAt: Date.now(),
					preferredPath: s.executionPreference,
				};
				result = { success: true, error: '' };
				return { ...s, favorites: [...s.favorites, newFav] };
			});
			return result;
		},
		removeFromFavorites: (id: string) =>
			commit((s) => ({
				...s,
				favorites: s.favorites.filter((f) => f.id !== id),
			})),
		loadFromFavorites: (query: string) =>
			update((s) => ({ ...s, editorContent: query })),
		isFavoriteNameExists: (name: string): boolean => {
			let exists = false;
			update((s) => {
				exists = s.favorites.some(
					(f) => f.name.toLowerCase() === name.toLowerCase(),
				);
				return s;
			});
			return exists;
		},
	};

	function addToHistory(item: Omit<QueryHistoryItem, 'id' | 'timestamp'>) {
		commit((s) => {
			const newItem: QueryHistoryItem = {
				...item,
				id: generateId(),
				timestamp: Date.now(),
			};
			const newHistory = [newItem, ...s.history].slice(0, 50);
			return { ...s, history: newHistory };
		});
	}
}

export const consoleStore = createConsoleStore();
