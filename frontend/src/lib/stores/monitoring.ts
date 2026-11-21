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
import {
	streamMonitoring,
	type MonitoringStreamStatus,
} from '$services/monitoringStream';
import { t } from '$i18n';
import { storage } from '$utils/storage';
import { rangeQueryBounds, type TrendRange } from '$utils/timeWindow';
import type {
	DatabaseOverviewResponse,
	QueryStatsResponse,
	SearchStatsResponse,
	SystemResourceResponse,
} from '$services/statistics';

/** Poll interval for the monitoring page; also the store default. */
export const MONITOR_POLL_INTERVAL_MS = 10_000;

/** Live-stream snapshot cadence requested from the statistics SSE endpoint. */
export const MONITOR_STREAM_INTERVAL_MS = 2_000;

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
	/** Live statistics stream state; polling stays on as the fallback. */
	streamStatus: MonitoringStreamStatus;
	/** Selected trend range; presets window the timeseries client-side. */
	timeRange: TrendRange;
}

export interface Thresholds {
	cpuPercent: number;
	memPercent: number;
	slowMs: number;
	cpuEnabled: boolean;
	memEnabled: boolean;
	slowEnabled: boolean;
}

const THRESHOLDS_STORAGE_KEY = 'monitor.thresholds';

const DEFAULT_THRESHOLDS: Thresholds = {
	cpuPercent: 80,
	memPercent: 80,
	slowMs: 500,
	cpuEnabled: true,
	memEnabled: true,
	slowEnabled: true,
};

function loadThresholds(): Thresholds {
	const fallback = { ...DEFAULT_THRESHOLDS };
	const parsed = storage.get<Partial<Thresholds>>(THRESHOLDS_STORAGE_KEY);
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
		cpuEnabled:
			typeof parsed.cpuEnabled === 'boolean'
				? parsed.cpuEnabled
				: DEFAULT_THRESHOLDS.cpuEnabled,
		memEnabled:
			typeof parsed.memEnabled === 'boolean'
				? parsed.memEnabled
				: DEFAULT_THRESHOLDS.memEnabled,
		slowEnabled:
			typeof parsed.slowEnabled === 'boolean'
				? parsed.slowEnabled
				: DEFAULT_THRESHOLDS.slowEnabled,
	};
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
		streamStatus: 'off',
		timeRange: 'live',
	});

	let timer: ReturnType<typeof setInterval> | null = null;
	let streamAbort: AbortController | null = null;
	let retryTimer: ReturnType<typeof setTimeout> | null = null;
	let retryDelayMs = 2_000;
	const MAX_RETRY_DELAY_MS = 30_000;

	/** One refresh pass; a failed pass keeps the last good payloads. */
	async function refresh() {
		const { paused, timeRange, streamStatus } = get({ subscribe });
		if (paused) return;
		// The live SSE stream already pushes overview (which embeds system and
		// database). Skip those three while it is live to avoid duplicate
		// requests; polling still covers the remaining panels and acts as the
		// full fallback whenever the stream is unavailable.
		const live = streamStatus === 'live';
		const queryBounds = rangeQueryBounds(timeRange);
		update((s) => ({ ...s, loading: true }));
		const settled = await Promise.allSettled([
			live ? Promise.resolve(undefined) : statisticsService.overview(),
			live ? Promise.resolve(undefined) : statisticsService.system(),
			live ? Promise.resolve(undefined) : statisticsService.database(),
			statisticsService.queries(
				queryBounds ? { from: queryBounds.from, to: queryBounds.to } : undefined,
			),
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
			if (overview.status === 'fulfilled' && overview.value !== undefined)
				next.overview = overview.value;
			if (system.status === 'fulfilled' && system.value !== undefined)
				next.system = system.value;
			if (database.status === 'fulfilled' && database.value !== undefined)
				next.database = database.value;
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

	/**
	 * Merge one streamed overview snapshot into the store.
	 *
	 * The overview embeds the system/database panels, so live updates also
	 * keep those fresh between polling passes.
	 */
	function applyStreamSnapshot(overview: OverviewResponse) {
		update((s) => ({
			...s,
			snapshots: {
				...s.snapshots,
				overview,
				system: overview.system ?? s.snapshots.system,
				database: overview.database ?? s.snapshots.database,
			},
			lastRefreshAt: Date.now(),
		}));
	}

	/** Start the live snapshot stream; polling keeps running as fallback. */
	async function startStreaming() {
		if (streamAbort !== null) return;
		const controller = new AbortController();
		streamAbort = controller;
		const outcome = await streamMonitoring({
			intervalMs: MONITOR_STREAM_INTERVAL_MS,
			signal: controller.signal,
			onSnapshot: applyStreamSnapshot,
			onStatus: (status) =>
				update((s) => ({ ...s, streamStatus: status })),
		});
		if (streamAbort !== controller) return;
		streamAbort = null;
		if (outcome.cancelled) {
			update((s) => ({ ...s, streamStatus: 'stopped' }));
			return;
		}
		if (!outcome.streamError) {
			retryDelayMs = 2_000;
			update((s) => ({ ...s, streamStatus: 'stopped' }));
			return;
		}
		update((s) => ({ ...s, streamStatus: 'unavailable' }));
		retryTimer = setTimeout(() => {
			retryTimer = null;
			void startStreaming();
		}, retryDelayMs);
		retryDelayMs = Math.min(retryDelayMs * 2, MAX_RETRY_DELAY_MS);
	}

	function stopStreaming() {
		if (retryTimer !== null) {
			clearTimeout(retryTimer);
			retryTimer = null;
		}
		streamAbort?.abort();
	}

	return {
		subscribe,
		refresh,
		startPolling,
		stopPolling,
		startStreaming,
		stopStreaming,
		/** Start both transports: live stream plus polling fallback. */
		start: () => {
			startPolling();
			void startStreaming();
		},
		stop: () => {
			stopPolling();
			stopStreaming();
		},
		setPaused: (paused: boolean) => update((s) => ({ ...s, paused })),
		/** Select the trend range; preset ranges immediately refetch stats. */
		setTimeRange: (range: TrendRange) => {
			update((s) => ({ ...s, timeRange: range }));
			void refresh();
		},
		setThresholds: (next: Thresholds) => {
			update((s) => ({ ...s, thresholds: next }));
			persistThresholds(next);
		},
	};
}

export const monitoringStore = createMonitoringStore();
