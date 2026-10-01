import { writable, get } from 'svelte/store';
import { statisticsService, type OverviewResponse } from '$services/statistics';
import type {
	DatabaseOverviewResponse,
	QueryStatsResponse,
	SearchStatsResponse,
	SystemResourceResponse
} from '$services/statistics';

/** Poll interval for the monitoring page; also the store default. */
export const MONITOR_POLL_INTERVAL_MS = 10_000;

export interface MonitoringSnapshots {
	overview: OverviewResponse | null;
	system: SystemResourceResponse | null;
	database: DatabaseOverviewResponse | null;
	queries: QueryStatsResponse | null;
	search: SearchStatsResponse | null;
	transaction: Record<string, unknown> | null;
	sync: Record<string, unknown> | null;
}

interface MonitoringState {
	snapshots: MonitoringSnapshots;
	lastRefreshAt: number | null;
	loading: boolean;
	paused: boolean;
	error: string | null;
}

const emptySnapshots = (): MonitoringSnapshots => ({
	overview: null,
	system: null,
	database: null,
	queries: null,
	search: null,
	transaction: null,
	sync: null
});

function createMonitoringStore() {
	const { subscribe, update } = writable<MonitoringState>({
		snapshots: emptySnapshots(),
		lastRefreshAt: null,
		loading: false,
		paused: false,
		error: null
	});

	let timer: ReturnType<typeof setInterval> | null = null;

	/** One refresh pass; a failed pass keeps the last good payloads. */
	async function refresh() {
		const { paused } = get({ subscribe });
		if (paused) return;
		update((s) => ({ ...s, loading: true }));
		const settled = await Promise.allSettled([
			statisticsService.overview(),
			statisticsService.system(),
			statisticsService.database(),
			statisticsService.queries(),
			statisticsService.search(),
			statisticsService.transactionMetrics(),
			statisticsService.syncStatus()
		]);
		const [overview, system, database, queries, search, transaction, sync] = settled;
		const errors = settled
			.filter((r): r is PromiseRejectedResult => r.status === 'rejected')
			.map((r) => (r.reason instanceof Error ? r.reason.message : 'Request failed'));
		update((s) => {
			const next = { ...s.snapshots };
			if (overview.status === 'fulfilled') next.overview = overview.value;
			if (system.status === 'fulfilled') next.system = system.value;
			if (database.status === 'fulfilled') next.database = database.value;
			if (queries.status === 'fulfilled') next.queries = queries.value;
			if (search.status === 'fulfilled') next.search = search.value;
			if (transaction.status === 'fulfilled')
				next.transaction = transaction.value as Record<string, unknown>;
			if (sync.status === 'fulfilled') next.sync = sync.value as Record<string, unknown>;
			const failed = errors.length > 0;
			return {
				...s,
				snapshots: next,
				loading: false,
				lastRefreshAt: failed && s.lastRefreshAt !== null ? s.lastRefreshAt : Date.now(),
				error: failed ? errors[0] : null
			};
		});
	}

	function startPolling(intervalMs: number = MONITOR_POLL_INTERVAL_MS) {
		stopPolling();
		void refresh();
		timer = setInterval(() => void refresh(), intervalMs);
	}

	function stopPolling() {
		if (timer !== null) {
			clearInterval(timer);
			timer = null;
		}
	}

	return {
		subscribe,
		refresh,
		startPolling,
		stopPolling,
		setPaused: (paused: boolean) =>
			update((s) => ({ ...s, paused }))
	};
}

export const monitoringStore = createMonitoringStore();
