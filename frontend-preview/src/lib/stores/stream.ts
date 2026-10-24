import { writable } from 'svelte/store';
import type { QueryError } from '$types/query';
import { streamQuery } from '$services/streamQuery';
import { getStreamEligibility } from '$utils/gql';
import { t } from '$i18n';

export type StreamStatus =
	'idle' | 'connecting' | 'receiving' | 'completed' | 'failed' | 'cancelled';

export type StreamCardStatus =
	'pending' | 'receiving' | 'completed' | 'failed' | 'cancelled';

export interface StreamCardState {
	index: number;
	query: string;
	columns: string[];
	rows: Record<string, unknown>[];
	receivedCount: number;
	reportedTotal: number | null;
	executionTime: number;
	status: StreamCardStatus;
	error: QueryError | null;
}

export interface StreamState {
	status: StreamStatus;
	query: string;
	batch: boolean;
	cards: StreamCardState[];
	executionTime: number;
	error: QueryError | null;
	startedAt: number;
	firstRowAt: number | null;
	doneReceived: boolean;
}

export interface StreamRunOptions {
	parameters: Record<string, unknown>;
	sessionVariables: Record<string, unknown>;
	onCardUpdate?: (cards: StreamCardState[]) => void;
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

let streamController: AbortController | null = null;
let streamGeneration = 0;

export function createStreamStore() {
	const { subscribe, update } = writable<StreamState>(idleStream());

	function abortActive() {
		streamController?.abort();
		streamController = null;
	}

	function getGeneration(): number {
		return streamGeneration;
	}

	function bumpGeneration(): number {
		streamGeneration += 1;
		return streamGeneration;
	}

	function isCurrent(generation: number): boolean {
		return generation === streamGeneration;
	}

	async function run(
		rawText: string,
		options: StreamRunOptions,
	): Promise<{ cancelled: boolean; doneReceived: boolean; streamError: { code: string; message: string } | null; executionTimeMs: number }> {
		const eligibility = getStreamEligibility(rawText);
		if (!eligibility.eligible || eligibility.mode === null) {
			return { cancelled: false, doneReceived: false, streamError: { code: 'NOT_ELIGIBLE', message: 'Query not eligible for streaming' }, executionTimeMs: 0 };
		}

		const batch = eligibility.mode === 'batch';
		const cardQueries = batch
			? eligibility.statements
			: [eligibility.statement];

		const generation = bumpGeneration();
		const controller = new AbortController();
		streamController = controller;
		const startedAt = Date.now();

		update(() => ({
			...idleStream(),
			status: 'connecting',
			query: rawText,
			batch,
			cards: cardQueries.map((query, index) => emptyCard(index, query)),
			startedAt,
		}));

		const cardQuery = batch
			? {
					query: '',
					statements: cardQueries,
					parameters: options.parameters,
					sessionVariables: options.sessionVariables,
					failFast: true,
				}
			: { query: cardQueries[0] };

		try {
			const outcome = await streamQuery(
				{ ...cardQuery, signal: controller.signal },
				{
					onStatementBegin: ({ index }) => {
						if (!isCurrent(generation)) return;
						update((s) => {
							const cards = s.cards.map((card) =>
								card.index === index && card.status === 'pending'
									? { ...card, status: 'receiving' as StreamCardStatus }
									: card,
							);
							return { ...s, cards };
						});
					},
					onStatementEnd: (info) => {
						if (!isCurrent(generation)) return;
						update((s) => {
							const cards = s.cards.map((card) =>
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
														message: info.message ?? t('errors.statementFailed'),
													}),
										}
									: card,
							);
							return { ...s, cards };
						});
					},
					onSchema: (columns, stmt) => {
						if (!isCurrent(generation)) return;
						const target = stmt;
						update((s) => ({
							...s,
							cards: s.cards.map((card) =>
								card.index === target ? { ...card, columns } : card,
							),
						}));
					},
					onRow: (row, index, stmt) => {
						if (!isCurrent(generation)) return;
						const target = stmt;
						update((s) => {
							const cards = s.cards.map((card) => {
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
								status: 'receiving',
								cards,
								firstRowAt: s.firstRowAt ?? Date.now(),
							};
						});
					},
					onMetadata: (info) => {
						if (!isCurrent(generation)) return;
						const target = info.stmt;
						update((s) => ({
							...s,
							cards: s.cards.map((card) =>
								card.index === target
									? {
											...card,
											executionTime: info.executionTimeMs,
											reportedTotal: info.rowsReturned,
										}
									: card,
							),
						}));
					},
					onStreamError: (streamError) => {
						if (!isCurrent(generation)) return;
						const target = streamError.stmt;
						update((s) => ({
							...s,
							cards: s.cards.map((card) =>
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
						}));
					},
				},
			);

			if (!isCurrent(generation)) return { cancelled: true, doneReceived: false, streamError: null, executionTimeMs: 0 };
			if (streamController === controller) streamController = null;

			if (outcome.cancelled || controller.signal.aborted) {
				failOverall('STREAM_CANCELLED', t('errors.streamCancelled'), startedAt, true);
				return { cancelled: true, doneReceived: false, streamError: null, executionTimeMs: Date.now() - startedAt };
			}
			if (!outcome.doneReceived) {
				failOverall('STREAM_INTERRUPTED', t('errors.streamInterrupted'), startedAt, true);
				return { cancelled: false, doneReceived: false, streamError: { code: 'STREAM_INTERRUPTED', message: t('errors.streamInterrupted') }, executionTimeMs: Date.now() - startedAt };
			}
			if (outcome.streamError && !batch) {
				update((s) => ({
					...s,
					status: 'failed',
					doneReceived: true,
					executionTime: s.executionTime || outcome.executionTimeMs || Date.now() - startedAt,
				}));
				return { cancelled: false, doneReceived: true, streamError: outcome.streamError, executionTimeMs: outcome.executionTimeMs || Date.now() - startedAt };
			}

			update((s) => ({
				...s,
				status: 'completed',
				doneReceived: true,
				executionTime: s.executionTime || Date.now() - startedAt,
			}));
			return { cancelled: false, doneReceived: true, streamError: null, executionTimeMs: Date.now() - startedAt };
		} catch (error) {
			if (!isCurrent(generation)) return { cancelled: true, doneReceived: false, streamError: null, executionTimeMs: 0 };
			if (streamController === controller) streamController = null;
			if (controller.signal.aborted) {
				failOverall('STREAM_CANCELLED', t('errors.streamCancelled'), startedAt, true);
				return { cancelled: true, doneReceived: false, streamError: null, executionTimeMs: Date.now() - startedAt };
			}
			failOverall(
				'STREAM_CONNECTION_ERROR',
				error instanceof Error ? error.message : t('errors.openStream'),
				startedAt,
			);
			return { cancelled: false, doneReceived: false, streamError: { code: 'STREAM_CONNECTION_ERROR', message: error instanceof Error ? error.message : t('errors.openStream') }, executionTimeMs: Date.now() - startedAt };
		}
	}

	function failOverall(
		code: string,
		message: string,
		startedAt: number,
		cancelled = false,
	) {
		const status = cancelled ? 'cancelled' : 'failed';
		update((s) => ({
			...s,
			status,
			executionTime: s.executionTime || Date.now() - startedAt,
			error: s.error ?? { code, message },
			cards: s.cards.map((card) =>
				card.status === 'receiving' ||
				(card.status === 'pending' && !s.batch)
					? {
							...card,
							status: status as StreamCardStatus,
							error: card.error ?? { code, message },
						}
					: card,
			),
		}));
	}

	function cancel() {
		abortActive();
	}

	function reset() {
		abortActive();
		update(() => idleStream());
	}

	return {
		subscribe,
		run,
		cancel,
		reset,
		abortActive,
		getGeneration,
		bumpGeneration,
		isCurrent,
	};
}

export const streamStore = createStreamStore();
