import { writable, get } from 'svelte/store';
import type { QueryError } from '$types/query';
import { cursorService } from '$services/cursor';
import { getStreamEligibility } from '$utils/gql';
import { t } from '$i18n';

export type CursorStatus =
	'idle' | 'opening' | 'open' | 'fetching' | 'exhausted' | 'failed';

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

let cursorGeneration = 0;

export function createCursorStore() {
	const { subscribe, update } = writable<CursorState>(idleCursor());

	function bumpGeneration(): number {
		cursorGeneration += 1;
		return cursorGeneration;
	}

	function isCurrent(generation: number): boolean {
		return generation === cursorGeneration;
	}

	async function open(rawText: string): Promise<boolean> {
		const eligibility = getStreamEligibility(rawText);
		if (!eligibility.eligible || eligibility.mode !== 'single') {
			return false;
		}

		const generation = bumpGeneration();
		const statement = eligibility.statement;
		const startedAt = Date.now();

		update(() => ({
			...idleCursor(),
			status: 'opening',
			query: statement,
			startedAt,
		}));

		try {
			const opened = await cursorService.open(statement);
			if (!isCurrent(generation)) {
				await cursorService.close(opened.cursorId);
				return true;
			}
			update((s) => ({
				...s,
				status: 'open',
				cursorId: opened.cursorId,
				columns: opened.columns,
			}));
			await fetchMore();
			return true;
		} catch (error) {
			if (!isCurrent(generation)) return true;
			update((s) => ({
				...s,
				status: 'failed',
				error: {
					code: 'CURSOR_OPEN_FAILED',
					message: error instanceof Error ? error.message : t('errors.openCursor'),
				},
			}));
			return true;
		}
	}

	async function fetchMore(): Promise<void> {
		const snapshot = get({ subscribe });
		if (snapshot.status !== 'open' || snapshot.cursorId === null) return;
		if (!snapshot.hasMore && snapshot.receivedCount > 0) return;

		const generation = cursorGeneration;
		const cursorId = snapshot.cursorId;
		const pageSize = snapshot.pageSize;

		update((s) => ({ ...s, status: 'fetching' }));

		try {
			const page = await cursorService.fetch(cursorId, pageSize);
			if (!isCurrent(generation)) return;
			update((s) => {
				const rows = [...s.rows, ...page.rows];
				return {
					...s,
					status: page.hasMore ? 'open' : 'exhausted',
					columns: page.columns.length > 0 ? page.columns : s.columns,
					rows,
					receivedCount: rows.length,
					hasMore: page.hasMore,
					error: null,
				};
			});
		} catch (error) {
			if (!isCurrent(generation)) return;
			update((s) => ({
				...s,
				status: 'failed',
				error: {
					code: 'CURSOR_FETCH_FAILED',
					message: error instanceof Error ? error.message : t('errors.fetchCursorPage'),
				},
			}));
		}
	}

	async function close(): Promise<void> {
		bumpGeneration();
		let cursorId: number | null = null;
		update((s) => {
			cursorId = s.cursorId;
			return s;
		});
		if (cursorId !== null) {
			await cursorService.close(cursorId);
		}
		update(() => idleCursor());
	}

	function reset(): void {
		bumpGeneration();
		update(() => idleCursor());
	}

	return {
		subscribe,
		open,
		fetchMore,
		close,
		reset,
	};
}

export const cursorStore = createCursorStore();
