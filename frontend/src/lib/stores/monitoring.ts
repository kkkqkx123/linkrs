import { writable, get } from 'svelte/store';
import {
	statisticsService,
	type OverviewResponse,
	type TransactionMetrics,
} from '$services/statistics';
import {
	retrievalService,
	type RetrievalSnapshot,
} from '$services/retrieval';
import { t } from '$i18n';
import { storage, getLegacyRaw } from '$utils/storage';
import type {
	DatabaseOverviewResponse,
	QueryStatsResponse,
	SearchStatsResponse,
	SystemResourceResponse,
} from '$services/statistics';

/** Poll interval for the monitoring page; also the store default. */
export const MONITOR_POLL_INTERVAL_MS = 10_000;

export interface MonitoringSnapshots {
	overview: OverviewResponse | null;
	system: SystemResourceResponse | null;
	database: DatabaseOverviewResponse | null;
	queries: QueryStatsResponse | null;
	search: SearchStatsResponse | null;
	transaction: TransactionMetrics | null;
	sync: Record<string, unknown> | null;
	retrieval: RetrievalSnapshot | null;
}

interface MonitoringState {
	snapshots: MonitoringSnapshots;
	lastRefreshAt: number | null;
	loading: boolean;
	paused: boolean;
	error: string | null;
	/** Alert thresholds; owned here so any panel can consume them. */
	thresholds: Thresholds;
}

export interface Thresholds {
	cpuPercent: number;
	memPercent: number;
	slowMs: number;
}

const THRESHOLDS_STORAGE_KEY = 'monitor.thresholds';
// Key used before storage access was unified; read once as a fallback.
const LEGACY_THRESHOLDS_STORAGE_KEY = 'linkrs_monitor_thresholds';

const DEFAULT_THRESHOLDS: Thresholds = {
	cpuPercent: 80,
	memPercent: 80,
	slowMs: 500,
};

function loadThresholds(): Thresholds {
	const fallback = { ...DEFAULT_THRESHOLDS };
	const parsed =
		storage.get<Partial<Thresholds>>(THRESHOLDS_STORAGE_KEY) ??
		parseLegacyThresholds();
	if (!parsed) return fallback;
	return {
		cpuPercent:
			typeof parsed.cpuPercent === 'number'
				? parsed.cpuPercent
				: DEFAULT_THRESHOLDS.cpuPercent,
		memPercent:
			typeof parsed.memPercent === 'number'
				? parsed.memPercent
				: DEFAULT_THRESHOLDS.memPercent,
		slowMs:
			typeof parsed.slowMs === 'number'
				? parsed.slowMs
				: DEFAULT_THRESHOLDS.slowMs,
	};
}

function parseLegacyThresholds(): Partial<Thresholds> | null {
	try {
		const raw = getLegacyRaw(LEGACY_THRESHOLDS_STORAGE_KEY);
		if (!raw) return null;
		return JSON.parse(raw) as Partial<Thresholds>;
	} catch {
		return null;
	}
}

function persistThresholds(thresholds: Thresholds): void {
	storage.set(THRESHOLDS_STORAGE_KEY, thresholds);
}

const emptySnapshots = (): MonitoringSnapshots => ({
	overview: null,
	system: null,
	database: null,
	queries: null,
	search: null,
	transaction: null,
	sync: null,
	retrieval: null,
});

function createMonitoringStore() {
	const { subscribe, update } = writable<MonitoringState>({
		snapshots: emptySnapshots(),
		lastRefreshAt: null,
		loading: false,
		paused: false,
		error: null,
		thresholds: loadThresholds(),
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
			statisticsService.syncStatus(),
			retrievalService.snapshot(),
		]);
		const [
			overview,
			system,
			database,
			queries,
			search,
			transaction,
			sync,
			retrieval,
		] = settled;
		const errors = settled
			.filter((r): r is PromiseRejectedResult => r.status === 'rejected')
			.map((r) =>
				r.reason instanceof Error
					? r.reason.message
					: t('notification.requestFailed'),
			);
		update((s) => {
			const next = { ...s.snapshots };
			if (overview.status === 'fulfilled') next.overview = overview.value;
			if (system.status === 'fulfilled') next.system = system.value;
			if (database.status === 'fulfilled') next.database = database.value;
			if (queries.status === 'fulfilled') next.queries = queries.value;
			if (search.status === 'fulfilled') next.search = search.value;
			if (transaction.status === 'fulfilled')
				next.transaction = transaction.value;
			if (sync.status === 'fulfilled')
				next.sync = sync.value as Record<string, unknown>;
			if (retrieval.status === 'fulfilled') next.retrieval = retrieval.value;
			const failed = errors.length > 0;
			return {
				...s,
				snapshots: next,
				loading: false,
				lastRefreshAt:
					failed && s.lastRefreshAt !== null ? s.lastRefreshAt : Date.now(),
				error: failed ? errors[0] : null,
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
		setPaused: (paused: boolean) => update((s) => ({ ...s, paused })),
		setThresholds: (next: Thresholds) => {
			update((s) => ({ ...s, thresholds: next }));
			persistThresholds(next);
		},
	};
}

export const monitoringStore = createMonitoringStore();
