<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { t } from '$i18n';
	import { goto } from '$app/navigation';
	import {
		monitoringStore,
		type MonitoringSnapshots,
	} from '$stores/monitoring';
	import { consoleStore } from '$stores/console';
	import {
		statisticsService,
		type QueryProfileDetailResponse,
	} from '$services/statistics';
	import {
		operationsService,
		type ActiveTransaction,
	} from '$services/operations';
	import { syncService } from '$services/sync';
	import TrendChart from '$components/common/TrendChart.svelte';
	import {
		formatBytes,
		formatCount,
		formatLatencyMs,
		formatLatencyUs,
		formatPercent,
		formatPermille,
		formatQps,
		formatUptimeSecs,
	} from '$utils/metricsFormat';

	let monitor = $state({
		snapshots: {
			overview: null,
			system: null,
			database: null,
			queries: null,
			search: null,
			transaction: null,
			sync: null,
		} as MonitoringSnapshots,
		lastRefreshAt: null as number | null,
		loading: false,
		paused: false,
		error: null as string | null,
	});

	let selectedTrace = $state<string | null>(null);
	let portrait = $state<QueryProfileDetailResponse | null>(null);
	let portraitLoading = $state(false);
	let portraitError = $state<string | null>(null);
	let copied = $state(false);

	const overview = $derived(monitor.snapshots.overview);
	const system = $derived(monitor.snapshots.system ?? overview?.system ?? null);
	const database = $derived(
		monitor.snapshots.database ?? overview?.database ?? null,
	);
	const queries = $derived(monitor.snapshots.queries);
	const search = $derived(monitor.snapshots.search);

	const trendPoints = $derived(overview?.timeseries ?? []);

	// Multi-series trend data for the TrendChart: queries, latency, and errors
	// per timestamp, each normalized independently inside the chart.
	const trendSeries = $derived.by(() => {
		const points = trendPoints;
		if (points.length === 0) return [];
		return [
			{
				key: 'queries',
				label: t('monitoring.qps'),
				color: '#3b82f6',
				values: points.map((p) => Number(p.queries) || 0),
			},
			{
				key: 'latency',
				label: t('monitoring.avgLatency'),
				color: '#14b8a6',
				values: points.map((p) => Number(p.avg_latency_ms) || 0),
			},
			{
				key: 'errors',
				label: t('monitoring.errors'),
				color: '#ef4444',
				values: points.map((p) => Number(p.errors) || 0),
			},
		];
	});

	const trendXLabels = $derived(
		trendPoints.map((p) => {
			const second = Number(p.second) || 0;
			if (!second) return '';
			return new Date(second * 1000).toLocaleTimeString([], {
				hour: '2-digit',
				minute: '2-digit',
			});
		}),
	);

	const memRatio = $derived.by(() => {
		const used = Number(system?.memory_usage?.used_bytes ?? NaN);
		const total = Number(system?.memory_usage?.total_bytes ?? NaN);
		if (!Number.isFinite(used) || !Number.isFinite(total) || total <= 0)
			return null;
		return used / total;
	});

	const slowRows = $derived(queries?.slow_queries ?? []);
	const patterns = $derived(queries?.top_patterns ?? []);
	const executors = $derived(queries?.executor_summary ?? []);
	const byIndex = $derived(search?.by_index ?? []);

	const cpuRatio = $derived.by(() => {
		const raw = Number(system?.cpu_usage_percent ?? NaN);
		return Number.isFinite(raw) ? raw / 100 : null;
	});

	function num(record: unknown, key: string): unknown {
		if (record && typeof record === 'object') {
			return (record as Record<string, unknown>)[key];
		}
		return undefined;
	}

	async function openPortrait(traceId: string | null | undefined) {
		if (!traceId) return;
		selectedTrace = traceId;
		portrait = null;
		portraitError = null;
		portraitLoading = true;
		copied = false;
		try {
			portrait = await statisticsService.queryProfile(traceId);
		} catch (err) {
			portraitError =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			portraitLoading = false;
		}
	}

	function closePortrait() {
		selectedTrace = null;
		portrait = null;
		portraitError = null;
	}

	async function copyTrace() {
		if (!selectedTrace) return;
		try {
			await navigator.clipboard.writeText(selectedTrace);
			copied = true;
		} catch {
			copied = false;
		}
	}

	function backToConsole() {
		if (portrait?.query) consoleStore.loadFromHistory(portrait.query);
		goto('/console');
	}

	function togglePaused() {
		monitoringStore.setPaused(!monitor.paused);
	}

	let opsLoading = $state(false);
	let opsError = $state<string | null>(null);
	let configText = $state('');
	let configSections = $state<Record<string, Record<string, unknown>>>({});
	let configDrafts = $state<Record<string, string>>({});
	let configBusy = $state<Record<string, boolean>>({});
	let configMessage = $state<string | null>(null);
	let activeTransactions = $state<ActiveTransaction[]>([]);

	let syncLoading = $state(false);
	let syncError = $state<string | null>(null);
	let syncMessage = $state<string | null>(null);
	let syncDiagnostics = $state('');
	let syncStatusText = $state('');
	let deadLettersText = $state('');
	let degradedText = $state('');
	let retentionText = $state('');
	let clearTarget = $state('');
	let clearIndexId = $state('');
	let clearGeneration = $state('');
	let clearStartLsn = $state('');
	let clearEndLsn = $state('');

	function configEntryKey(section: string, key: string): string {
		return `${section}.${key}`;
	}

	function parseDraftValue(raw: string): unknown {
		const trimmed = raw.trim();
		if (trimmed === '') return '';
		try {
			return JSON.parse(trimmed);
		} catch {
			return raw;
		}
	}

	async function loadOps() {
		opsLoading = true;
		opsError = null;
		configMessage = null;
		try {
			const [config, txns] = await Promise.all([
				operationsService.config(),
				operationsService.transactions(),
			]);
			try {
				configText = JSON.stringify(config, null, 2);
			} catch {
				configText = String(config ?? '');
			}
			if (config && typeof config === 'object' && !Array.isArray(config)) {
				const grouped: Record<string, Record<string, unknown>> = {};
				const drafts: Record<string, string> = {};
				for (const [section, values] of Object.entries(
					config as Record<string, unknown>,
				)) {
					if (values && typeof values === 'object' && !Array.isArray(values)) {
						const entries = values as Record<string, unknown>;
						grouped[section] = entries;
						for (const [key, value] of Object.entries(entries)) {
							const entry = configEntryKey(section, key);
							drafts[entry] =
								typeof value === 'string' ? value : JSON.stringify(value ?? null);
						}
					}
				}
				configSections = grouped;
				configDrafts = drafts;
			} else {
				configSections = {};
				configDrafts = {};
			}
			activeTransactions = txns;
		} catch (err) {
			opsError = err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			opsLoading = false;
		}
	}

	async function saveConfigKey(section: string, key: string) {
		const entry = configEntryKey(section, key);
		configBusy = { ...configBusy, [entry]: true };
		configMessage = null;
		try {
			const value = parseDraftValue(configDrafts[entry] ?? '');
			await operationsService.updateConfigKey(section, key, value);
			configMessage = t('monitoring.configSaved', { id: entry });
			await loadOps();
		} catch (err) {
			opsError = err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			configBusy = { ...configBusy, [entry]: false };
		}
	}

	async function resetConfigKey(section: string, key: string) {
		const entry = configEntryKey(section, key);
		if (!confirm(t('monitoring.confirmReset', { id: entry }))) return;
		configBusy = { ...configBusy, [entry]: true };
		configMessage = null;
		try {
			await operationsService.resetConfigKey(section, key);
			configMessage = t('monitoring.configReset', { id: entry });
			await loadOps();
		} catch (err) {
			opsError = err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			configBusy = { ...configBusy, [entry]: false };
		}
	}

	function stringifyPayload(payload: unknown): string {
		try {
			return JSON.stringify(payload, null, 2);
		} catch {
			return String(payload ?? '');
		}
	}

	async function loadSyncDiagnostics() {
		syncLoading = true;
		syncError = null;
		syncMessage = null;
		try {
			const [status, diagnostics, deadLetters, degraded, retention] =
				await Promise.all([
					syncService.status(),
					syncService.diagnostics(),
					syncService.deadLetters({ limit: 20 }),
					syncService.degradedRanges(),
					syncService.retentionStatus(),
				]);
			syncStatusText = stringifyPayload(status);
			syncDiagnostics = stringifyPayload(diagnostics);
			deadLettersText = stringifyPayload(deadLetters);
			degradedText = stringifyPayload(degraded);
			retentionText = stringifyPayload(retention);
		} catch (err) {
			syncError = err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			syncLoading = false;
		}
	}

	async function retryOutbox() {
		syncLoading = true;
		syncError = null;
		try {
			const result = await syncService.retryOutbox();
			syncMessage = stringifyPayload(result);
			await loadSyncDiagnostics();
		} catch (err) {
			syncError = err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			syncLoading = false;
		}
	}

	async function requeueDeadLetters() {
		if (!confirm(t('monitoring.confirmRequeue'))) return;
		syncLoading = true;
		syncError = null;
		try {
			const result = await syncService.requeue({ limit: 100 });
			syncMessage = stringifyPayload(result);
			await loadSyncDiagnostics();
		} catch (err) {
			syncError = err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			syncLoading = false;
		}
	}

	async function runRetention() {
		syncLoading = true;
		syncError = null;
		try {
			const result = await syncService.retentionRun();
			syncMessage = stringifyPayload(result);
			await loadSyncDiagnostics();
		} catch (err) {
			syncError = err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			syncLoading = false;
		}
	}

	async function clearDegradedRange() {
		const indexId = Number(clearIndexId);
		const generation = Number(clearGeneration);
		const startLsn = Number(clearStartLsn);
		const endLsn = Number(clearEndLsn);
		if (
			!clearTarget.trim() ||
			![indexId, generation, startLsn, endLsn].every(
				(n) => Number.isInteger(n) && n >= 0,
			)
		) {
			syncError = t('monitoring.clearDegradedInvalid');
			return;
		}
		if (!confirm(t('monitoring.confirmClearDegraded', { id: clearTarget })))
			return;
		syncLoading = true;
		syncError = null;
		try {
			const result = await syncService.clearDegraded({
				target: clearTarget.trim(),
				index_id: indexId,
				generation,
				start_lsn: startLsn,
				end_lsn: endLsn,
			});
			syncMessage = stringifyPayload(result);
			await loadSyncDiagnostics();
		} catch (err) {
			syncError = err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			syncLoading = false;
		}
	}

	async function killTransaction(id: number) {
		if (!confirm(t('monitoring.confirmKill', { id }))) return;
		try {
			await operationsService.killTransaction(id);
			await loadOps();
		} catch (err) {
			opsError = err instanceof Error ? err.message : t('notification.requestFailed');
		}
	}

	interface Thresholds {
		cpuPercent: number;
		memPercent: number;
		slowMs: number;
	}

	function loadThresholds(): Thresholds {
		const fallback: Thresholds = { cpuPercent: 80, memPercent: 80, slowMs: 500 };
		try {
			const raw = localStorage.getItem('graphdb_monitor_thresholds');
			if (!raw) return fallback;
			const parsed = JSON.parse(raw) as Partial<Thresholds>;
			return {
				cpuPercent:
					typeof parsed.cpuPercent === 'number' ? parsed.cpuPercent : fallback.cpuPercent,
				memPercent:
					typeof parsed.memPercent === 'number' ? parsed.memPercent : fallback.memPercent,
				slowMs: typeof parsed.slowMs === 'number' ? parsed.slowMs : fallback.slowMs,
			};
		} catch {
			return fallback;
		}
	}

	let thresholds = $state<Thresholds>({ cpuPercent: 80, memPercent: 80, slowMs: 500 });
	let thresholdsReady = $state(false);

	$effect(() => {
		if (!thresholdsReady) return;
		try {
			localStorage.setItem('graphdb_monitor_thresholds', JSON.stringify(thresholds));
		} catch {
			/* storage unavailable */
		}
	});

	const cpuExceeded = $derived(
		cpuRatio !== null && cpuRatio * 100 > thresholds.cpuPercent,
	);
	const memExceeded = $derived(
		memRatio !== null && memRatio * 100 > thresholds.memPercent,
	);

	onMount(() => {
		thresholds = loadThresholds();
		thresholdsReady = true;
		const unsub = monitoringStore.subscribe((s) => {
			monitor = {
				snapshots: s.snapshots as MonitoringSnapshots,
				lastRefreshAt: s.lastRefreshAt,
				loading: s.loading,
				paused: s.paused,
				error: s.error,
			};
		});
		monitoringStore.startPolling();
		return () => {
			unsub();
			monitoringStore.stopPolling();
		};
	});

	onDestroy(() => monitoringStore.stopPolling());
</script>

<div class="max-w-6xl mx-auto space-y-4 animate-fade-in pb-8">
	<div class="flex items-center justify-between flex-wrap gap-2">
		<h1 class="text-xl font-bold text-gray-800 dark:text-gray-100">
			{t('sidebar.monitoring')}
		</h1>
		<div class="flex items-center gap-2 text-sm">
			{#if monitor.lastRefreshAt}
				<span class="text-gray-500 dark:text-gray-400">
					{t('monitoring.lastRefresh')}: {new Date(
						monitor.lastRefreshAt,
					).toLocaleTimeString()}
				</span>
			{/if}
			<button
				class="px-3 py-1 rounded bg-gray-100 dark:bg-gray-700/50 text-gray-700 dark:text-gray-200 hover:bg-gray-200 dark:hover:bg-gray-700 cursor-pointer"
				onclick={togglePaused}
			>
				{monitor.paused ? t('monitoring.resume') : t('monitoring.pause')}
			</button>
			<button
				class="px-3 py-1 rounded bg-blue-500 hover:bg-blue-600 text-white disabled:opacity-50 cursor-pointer"
				onclick={() => monitoringStore.refresh()}
				disabled={monitor.loading}
			>
				{monitor.loading ? t('monitoring.loading') : t('common.refresh')}
			</button>
		</div>
	</div>

	<section
		class="bg-white dark:bg-[#1C2333] rounded-xl px-5 py-3 border border-gray-100 dark:border-gray-700/50 shadow-sm flex items-center gap-4 flex-wrap text-sm"
	>
		<span class="font-medium text-gray-700 dark:text-gray-300">{t('monitoring.thresholds')}</span>
		<label class="flex items-center gap-1 text-xs text-gray-500 dark:text-gray-400">
			{t('monitoring.cpu')} &gt;
			<input
				type="number"
				min="1"
				max="100"
				class="w-16 px-2 py-1 border border-gray-300 dark:border-gray-600 rounded text-xs font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
				bind:value={thresholds.cpuPercent}
			/>%
		</label>
		<label class="flex items-center gap-1 text-xs text-gray-500 dark:text-gray-400">
			{t('monitoring.memory')} &gt;
			<input
				type="number"
				min="1"
				max="100"
				class="w-16 px-2 py-1 border border-gray-300 dark:border-gray-600 rounded text-xs font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
				bind:value={thresholds.memPercent}
			/>%
		</label>
		<label class="flex items-center gap-1 text-xs text-gray-500 dark:text-gray-400">
			{t('monitoring.slowQueries')} &gt;
			<input
				type="number"
				min="1"
				class="w-20 px-2 py-1 border border-gray-300 dark:border-gray-600 rounded text-xs font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
				bind:value={thresholds.slowMs}
			/>ms
		</label>
		{#if cpuExceeded || memExceeded}
			<span class="text-xs font-medium text-red-500 dark:text-red-400">
				{t('monitoring.thresholdExceeded')}
			</span>
		{/if}
	</section>

	{#if monitor.error}
		<div
			class="p-2 bg-yellow-50 dark:bg-yellow-900/20 border border-yellow-200 dark:border-yellow-800 rounded text-xs text-yellow-700 dark:text-yellow-300"
		>
			{t('monitoring.loadFailed')}: {monitor.error}
		</div>
	{/if}

	<!-- Overview -->
	<section
		class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
	>
		<h2 class="font-semibold text-gray-800 dark:text-gray-100 mb-3">
			{t('monitoring.overview')}
		</h2>
		<div class="grid grid-cols-2 md:grid-cols-4 gap-3 text-sm">
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('common.status')}
				</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{system ? t('common.statusHealthy') : t('monitoring.noData')}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.connections')}
				</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatCount(system?.connections?.active)} / {formatCount(
						system?.connections?.max,
					)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.uptime')}
				</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatUptimeSecs(system?.uptime_secs)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.qps')}
				</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatQps(database?.performance?.queries_per_second)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.avgLatency')}
				</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatLatencyMs(database?.performance?.avg_latency_ms)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					p50 / p95 / p99
				</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatLatencyUs(overview?.query_latency_us?.p50)} /
					{formatLatencyUs(overview?.query_latency_us?.p95)} /
					{formatLatencyUs(overview?.query_latency_us?.p99)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.errors')}
				</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatCount(
						overview?.errors?.total ?? database?.performance?.error_total,
					)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.totalQueries')}
				</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatCount(
						database?.performance?.total_queries ?? queries?.total_queries,
					)}
				</div>
			</div>
		</div>
		{#if trendPoints.length > 0}
		 <div class="mt-4">
		  <div class="text-xs text-gray-500 dark:text-gray-400 mb-1">
		   {t('monitoring.trend')}: {formatCount(
		    trendPoints.reduce((a, p) => a + (Number(p.queries) || 0), 0),
		   )}
		   {t('monitoring.queriesAvg')}
		   {formatLatencyMs(
		    trendPoints.reduce(
		     (a, p) => a + (Number(p.avg_latency_ms) || 0),
		     0,
		    ) / Math.max(1, trendPoints.length),
		   )}
		  </div>
		  <TrendChart series={trendSeries} xLabels={trendXLabels} height={120} />
		 </div>
		{/if}
	</section>

	<!-- Resources -->
	<section
		class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
	>
		<h2 class="font-semibold text-gray-800 dark:text-gray-100 mb-3">
			{t('monitoring.resources')}
		</h2>
		<div class="grid grid-cols-2 md:grid-cols-4 gap-3 text-sm">
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.cpu')}
				</div>
				<div
					class="font-mono {cpuExceeded
						? 'text-red-500 dark:text-red-400 font-semibold'
						: 'text-gray-800 dark:text-gray-200'}"
				>
					{cpuRatio !== null ? formatPercent(cpuRatio) : '—'}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.memory')}
				</div>
				<div
					class="font-mono {memExceeded
						? 'text-red-500 dark:text-red-400 font-semibold'
						: 'text-gray-800 dark:text-gray-200'}"
				>
					{formatBytes(system?.memory_usage?.used_bytes)} / {formatBytes(
						system?.memory_usage?.total_bytes,
					)}
					{#if memRatio !== null}({formatPercent(memRatio)}){/if}
				</div>
				{#if memRatio !== null}
					<div
						class="h-1.5 mt-1 bg-gray-100 dark:bg-gray-700 rounded overflow-hidden"
					>
						<div
							class="h-full bg-blue-500"
							style="width: {Math.min(100, memRatio * 100)}%"
						></div>
					</div>
				{/if}
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('common.processMemory')}
				</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatBytes(system?.process_memory_bytes)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.dataDir')}
				</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatBytes(system?.data_dir_size_bytes)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.walDir')}
				</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatBytes(system?.wal_dir_size_bytes)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.maxFds')}
				</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatCount(system?.max_file_descriptors)}
				</div>
			</div>
		</div>
	</section>

	<!-- Queries -->
	<section
		class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
	>
		<h2 class="font-semibold text-gray-800 dark:text-gray-100 mb-3">
			{t('monitoring.queries')}
		</h2>
		<div class="grid grid-cols-2 md:grid-cols-4 gap-3 text-sm mb-4">
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.totalQueries')}
				</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatCount(queries?.total_queries)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.active')}
				</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatCount(database?.performance?.active_queries)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.avgLatency')}
				</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatLatencyUs(queries?.latency_percentiles_us?.avg)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.errors')}
				</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatCount(queries?.error_total)}
				</div>
			</div>
		</div>

		{#if queries}
			<div class="grid md:grid-cols-2 gap-4 mb-4 text-sm">
				<div>
					<h3 class="text-xs font-medium text-gray-500 dark:text-gray-400 mb-1">
						{t('monitoring.errorsByType')}
					</h3>
					{#if Object.keys(queries.errors_by_type ?? {}).length === 0}
						<p class="text-gray-400">{t('monitoring.noData')}</p>
					{:else}
						<ul class="space-y-1">
							{#each Object.entries(queries.errors_by_type ?? {}) as [kind, count] (kind)}
								<li class="flex justify-between font-mono">
									<span class="text-gray-600 dark:text-gray-300">{kind}</span>
									<span class="text-gray-800 dark:text-gray-200"
										>{formatCount(count)}</span
									>
								</li>
							{/each}
						</ul>
					{/if}
				</div>
				<div>
					<h3 class="text-xs font-medium text-gray-500 dark:text-gray-400 mb-1">
						{t('monitoring.errorsByPhase')}
					</h3>
					{#if Object.keys(queries.errors_by_phase ?? {}).length === 0}
						<p class="text-gray-400">{t('monitoring.noData')}</p>
					{:else}
						<ul class="space-y-1">
							{#each Object.entries(queries.errors_by_phase ?? {}) as [phase, count] (phase)}
								<li class="flex justify-between font-mono">
									<span class="text-gray-600 dark:text-gray-300">{phase}</span>
									<span class="text-gray-800 dark:text-gray-200"
										>{formatCount(count)}</span
									>
								</li>
							{/each}
						</ul>
					{/if}
				</div>
			</div>
		{/if}

		<h3 class="text-xs font-medium text-gray-500 dark:text-gray-400 mb-1">
			{t('monitoring.topPatterns')}
		</h3>
		<div class="overflow-x-auto mb-4">
			{#if patterns.length === 0}
				<p class="text-sm text-gray-400">{t('monitoring.noData')}</p>
			{:else}
				<table class="min-w-full text-sm">
					<thead>
						<tr class="text-left text-xs text-gray-500 dark:text-gray-400">
							<th class="py-1 pr-3">{t('monitoring.pattern')}</th>
							<th class="py-1 pr-3">{t('monitoring.count')}</th>
							<th class="py-1 pr-3">{t('monitoring.avg')}</th>
							<th class="py-1 pr-3">p95</th>
							<th class="py-1 pr-3">p99</th>
							<th class="py-1 pr-3">{t('monitoring.errorRate')}</th>
						</tr>
					</thead>
					<tbody>
						{#each patterns as p (p.normalized_query)}
							<tr
								class="border-t border-gray-100 dark:border-gray-700/50 font-mono"
							>
								<td
									class="py-1 pr-3 max-w-72 truncate text-gray-700 dark:text-gray-200"
								>
									{p.normalized_query}
								</td>
								<td class="py-1 pr-3">{formatCount(p.execution_count)}</td>
								<td class="py-1 pr-3">{formatLatencyMs(p.avg_duration_ms)}</td>
								<td class="py-1 pr-3">{formatLatencyMs(p.p95_duration_ms)}</td>
								<td class="py-1 pr-3">{formatLatencyMs(p.p99_duration_ms)}</td>
								<td class="py-1 pr-3">{formatPercent(p.error_rate)}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			{/if}
		</div>

		<h3 class="text-xs font-medium text-gray-500 dark:text-gray-400 mb-1">
			{t('monitoring.executors')}
		</h3>
		<div class="overflow-x-auto mb-4">
			{#if executors.length === 0}
				<p class="text-sm text-gray-400">{t('monitoring.noData')}</p>
			{:else}
				<table class="min-w-full text-sm">
					<thead>
						<tr class="text-left text-xs text-gray-500 dark:text-gray-400">
							<th class="py-1 pr-3">{t('monitoring.executor')}</th>
							<th class="py-1 pr-3">{t('monitoring.count')}</th>
							<th class="py-1 pr-3">{t('monitoring.totalTime')}</th>
							<th class="py-1 pr-3">{t('monitoring.totalRows')}</th>
						</tr>
					</thead>
					<tbody>
						{#each executors as e (e.executor_type)}
							<tr
								class="border-t border-gray-100 dark:border-gray-700/50 font-mono"
							>
								<td class="py-1 pr-3 text-gray-700 dark:text-gray-200"
									>{e.executor_type}</td
								>
								<td class="py-1 pr-3">{formatCount(e.count)}</td>
								<td class="py-1 pr-3">{formatLatencyMs(e.total_time_ms)}</td>
								<td class="py-1 pr-3">{formatCount(e.total_rows)}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			{/if}
		</div>

		<h3 class="text-xs font-medium text-gray-500 dark:text-gray-400 mb-1">
			{t('monitoring.slowQueries')}
		</h3>
		<div class="overflow-x-auto">
			{#if slowRows.length === 0}
				<p class="text-sm text-gray-400">{t('monitoring.noData')}</p>
			{:else}
				<table class="min-w-full text-sm">
					<thead>
						<tr class="text-left text-xs text-gray-500 dark:text-gray-400">
							<th class="py-1 pr-3">{t('monitoring.query')}</th>
							<th class="py-1 pr-3">{t('monitoring.duration')}</th>
							<th class="py-1 pr-3">{t('monitoring.trace')}</th>
						</tr>
					</thead>
					<tbody>
						{#each slowRows as row (row.trace_id)}
							<tr
								class="border-t border-gray-100 dark:border-gray-700/50 {Number(
									row.duration_ms,
								) > thresholds.slowMs
									? 'bg-red-50 dark:bg-red-900/10'
									: ''}"
							>
								<td
									class="py-1 pr-3 max-w-80 truncate font-mono text-gray-700 dark:text-gray-200"
								>
									<button
										class="hover:underline cursor-pointer text-left"
										onclick={() => openPortrait(row.trace_id)}
									>
										{row.query}
									</button>
								</td>
								<td class="py-1 pr-3 font-mono"
									>{formatLatencyMs(row.duration_ms)}</td
								>
								<td class="py-1 pr-3 font-mono text-xs text-gray-500"
									>{row.trace_id}</td
								>
							</tr>
						{/each}
					</tbody>
				</table>
			{/if}
		</div>
	</section>

	<!-- Storage -->
	<section
		class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
	>
		<h2 class="font-semibold text-gray-800 dark:text-gray-100 mb-3">
			{t('monitoring.storage')}
		</h2>
		<div class="grid grid-cols-2 md:grid-cols-4 gap-3 text-sm">
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.spaces')}
				</div>
				<div class="font-mono">{formatCount(database?.spaces?.count)}</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.vertices')}
				</div>
				<div class="font-mono">
					{formatCount(database?.spaces?.total_vertices)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.edges')}
				</div>
				<div class="font-mono">
					{formatCount(database?.spaces?.total_edges)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.totalSize')}
				</div>
				<div class="font-mono">
					{formatBytes(database?.storage?.total_size_bytes)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.indexSize')}
				</div>
				<div class="font-mono">
					{formatBytes(database?.storage?.index_size_bytes)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.dataSize')}
				</div>
				<div class="font-mono">
					{formatBytes(database?.storage?.data_size_bytes)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.fragmentation')}
				</div>
				<div class="font-mono">
					{formatPermille(database?.storage?.fragmentation_permille)} ({formatBytes(
						database?.storage?.wasted_bytes,
					)})
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.tombstones')}
				</div>
				<div class="font-mono">
					{formatCount(database?.storage?.tombstone_count)} ({formatBytes(
						database?.storage?.tombstone_memory_bytes,
					)})
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.dirtyPages')}
				</div>
				<div class="font-mono">
					{formatCount(database?.storage?.dirty_pages)} / {formatCount(
						database?.storage?.dirty_pages_total,
					)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{t('monitoring.checkpointSuccess')}
				</div>
				<div class="font-mono">
					{formatCount(database?.storage?.checkpoint?.success_count)} / {formatCount(
						database?.storage?.checkpoint?.failure_count,
					)}
				</div>
			</div>
		</div>
	</section>

	<!-- Transactions + sync + search -->
	<div class="grid md:grid-cols-3 gap-4">
		<section
			class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
		>
			<h2 class="font-semibold text-gray-800 dark:text-gray-100 mb-3">
				{t('monitoring.transactions')}
			</h2>
			<div class="space-y-1 text-sm font-mono">
				<div class="flex justify-between">
					<span class="text-gray-500">{t('monitoring.begun')}</span>
					<span
						>{formatCount(
							overview?.transaction?.begun ??
								num(monitor.snapshots.transaction, 'begun'),
						)}</span
					>
				</div>
				<div class="flex justify-between">
					<span class="text-gray-500">{t('monitoring.committed')}</span>
					<span
						>{formatCount(
							overview?.transaction?.committed ??
								num(monitor.snapshots.transaction, 'committed'),
						)}</span
					>
				</div>
				<div class="flex justify-between">
					<span class="text-gray-500">{t('monitoring.rolledBack')}</span>
					<span
						>{formatCount(
							overview?.transaction?.rolled_back ??
								num(monitor.snapshots.transaction, 'rolled_back'),
						)}</span
					>
				</div>
				<div class="flex justify-between">
					<span class="text-gray-500">{t('monitoring.active')}</span>
					<span
						>{formatCount(
							overview?.transaction?.active ??
								num(monitor.snapshots.transaction, 'active'),
						)}</span
					>
				</div>
			</div>
		</section>
		<section
			class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
		>
			<div class="flex items-center justify-between mb-3">
				<h2 class="font-semibold text-gray-800 dark:text-gray-100">
					{t('monitoring.sync')}
				</h2>
				<button
					class="px-3 py-1 text-xs rounded bg-blue-500 hover:bg-blue-600 text-white disabled:opacity-50 cursor-pointer"
					onclick={loadSyncDiagnostics}
					disabled={syncLoading}
				>
					{syncLoading ? t('monitoring.loading') : t('common.refresh')}
				</button>
			</div>
			<div class="space-y-1 text-sm font-mono mb-3">
				<div class="flex justify-between">
					<span class="text-gray-500">{t('common.status')}</span>
					<span
						>{(overview?.sync?.is_running ??
						num(monitor.snapshots.sync, 'is_running'))
							? t('common.running')
							: '—'}</span
					>
				</div>
				{#if syncStatusText}
					<pre
						class="font-mono bg-gray-50 dark:bg-gray-800/50 p-2 rounded border border-gray-200 dark:border-gray-700 overflow-auto max-h-32 text-gray-700 dark:text-gray-300">{syncStatusText}</pre
					>
				{/if}
				<div class="flex justify-between">
					<span class="text-gray-500">{t('monitoring.outbox')}</span>
					<span>
						{formatCount(
							overview?.sync?.outbox_pending ??
								num(monitor.snapshots.sync, 'outbox_pending'),
						)}
						/
						{formatCount(
							overview?.sync?.outbox_retries ??
								num(monitor.snapshots.sync, 'outbox_retries'),
						)}
						/
						{formatCount(
							overview?.sync?.outbox_dead_lettered ??
								num(monitor.snapshots.sync, 'outbox_dead_lettered'),
						)}
					</span>
				</div>
			</div>
			{#if syncError}
				<p class="text-xs text-red-500 mb-2">{syncError}</p>
			{/if}
			{#if syncMessage}
				<pre
					class="text-xs font-mono bg-gray-50 dark:bg-gray-800/50 p-2 rounded border border-gray-200 dark:border-gray-700 overflow-auto max-h-32 mb-2 text-gray-700 dark:text-gray-300">{syncMessage}</pre
				>
			{/if}
			<div class="flex flex-wrap gap-2 mb-3">
				<button
					class="px-2 py-1 text-xs rounded bg-blue-500 hover:bg-blue-600 text-white disabled:opacity-50 cursor-pointer"
					onclick={retryOutbox}
					disabled={syncLoading}
				>
					{t('monitoring.retryOutbox')}
				</button>
				<button
					class="px-2 py-1 text-xs rounded bg-amber-500 hover:bg-amber-600 text-white disabled:opacity-50 cursor-pointer"
					onclick={requeueDeadLetters}
					disabled={syncLoading}
				>
					{t('monitoring.requeueDeadLetters')}
				</button>
				<button
					class="px-2 py-1 text-xs rounded bg-gray-500 hover:bg-gray-600 text-white disabled:opacity-50 cursor-pointer"
					onclick={runRetention}
					disabled={syncLoading}
				>
					{t('monitoring.runRetention')}
				</button>
			</div>
			<div class="grid md:grid-cols-2 gap-3 text-xs">
				<div>
					<h3 class="font-medium text-gray-500 dark:text-gray-400 mb-1">
						{t('monitoring.deadLetters')}
					</h3>
					{#if deadLettersText}
						<pre
							class="font-mono bg-gray-50 dark:bg-gray-800/50 p-2 rounded border border-gray-200 dark:border-gray-700 overflow-auto max-h-40 text-gray-700 dark:text-gray-300">{deadLettersText}</pre
						>
					{:else}
						<p class="text-gray-400">{t('monitoring.noData')}</p>
					{/if}
				</div>
				<div>
					<h3 class="font-medium text-gray-500 dark:text-gray-400 mb-1">
						{t('monitoring.degradedRanges')}
					</h3>
					{#if degradedText}
						<pre
							class="font-mono bg-gray-50 dark:bg-gray-800/50 p-2 rounded border border-gray-200 dark:border-gray-700 overflow-auto max-h-40 text-gray-700 dark:text-gray-300">{degradedText}</pre
						>
					{:else}
						<p class="text-gray-400">{t('monitoring.noData')}</p>
					{/if}
					<div class="grid grid-cols-2 gap-1 mt-2">
						<input
							class="px-2 py-1 text-xs rounded border border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-800"
							placeholder="target"
							bind:value={clearTarget}
						/>
						<input
							class="px-2 py-1 text-xs rounded border border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-800"
							placeholder="index_id"
							inputmode="numeric"
							bind:value={clearIndexId}
						/>
						<input
							class="px-2 py-1 text-xs rounded border border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-800"
							placeholder="generation"
							inputmode="numeric"
							bind:value={clearGeneration}
						/>
						<input
							class="px-2 py-1 text-xs rounded border border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-800"
							placeholder="start_lsn"
							inputmode="numeric"
							bind:value={clearStartLsn}
						/>
						<input
							class="px-2 py-1 text-xs rounded border border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-800 col-span-2"
							placeholder="end_lsn"
							inputmode="numeric"
							bind:value={clearEndLsn}
						/>
					</div>
					<button
						class="mt-2 px-2 py-1 text-xs rounded bg-red-500 hover:bg-red-600 text-white disabled:opacity-50 cursor-pointer"
						onclick={clearDegradedRange}
						disabled={syncLoading}
					>
						{t('monitoring.clearDegraded')}
					</button>
				</div>
				<div>
					<h3 class="font-medium text-gray-500 dark:text-gray-400 mb-1">
						{t('monitoring.diagnostics')}
					</h3>
					{#if syncDiagnostics}
						<pre
							class="font-mono bg-gray-50 dark:bg-gray-800/50 p-2 rounded border border-gray-200 dark:border-gray-700 overflow-auto max-h-40 text-gray-700 dark:text-gray-300">{syncDiagnostics}</pre
						>
					{:else}
						<p class="text-gray-400">{t('monitoring.noData')}</p>
					{/if}
				</div>
				<div>
					<h3 class="font-medium text-gray-500 dark:text-gray-400 mb-1">
						{t('monitoring.retention')}
					</h3>
					{#if retentionText}
						<pre
							class="font-mono bg-gray-50 dark:bg-gray-800/50 p-2 rounded border border-gray-200 dark:border-gray-700 overflow-auto max-h-40 text-gray-700 dark:text-gray-300">{retentionText}</pre
						>
					{:else}
						<p class="text-gray-400">{t('monitoring.noData')}</p>
					{/if}
				</div>
			</div>
		</section>
		<section
			class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
		>
			<h2 class="font-semibold text-gray-800 dark:text-gray-100 mb-3">
				{t('common.search')}
			</h2>
			<div class="space-y-1 text-sm font-mono">
				<div class="flex justify-between">
					<span class="text-gray-500">{t('monitoring.totalQueries')}</span>
					<span>{formatCount(search?.search?.total_queries)}</span>
				</div>
				<div class="flex justify-between">
					<span class="text-gray-500">{t('monitoring.avgLatency')}</span>
					<span>{formatLatencyMs(search?.search?.avg_latency_ms)}</span>
				</div>
				<div class="flex justify-between">
					<span class="text-gray-500">{t('monitoring.cacheHitRate')}</span>
					<span>{formatPercent(search?.cache?.hit_rate)}</span>
				</div>
			</div>
			{#if byIndex.length > 0}
				<h3
					class="text-xs font-medium text-gray-500 dark:text-gray-400 mt-3 mb-1"
				>
					{t('monitoring.byIndex')}
				</h3>
				<div class="overflow-x-auto">
					<table class="min-w-full text-xs font-mono">
						<tbody>
							{#each byIndex.slice(0, 8) as entry (entry.index)}
								<tr class="border-t border-gray-100 dark:border-gray-700/50">
									<td class="py-1 pr-2 max-w-40 truncate">{entry.index}</td>
									<td class="py-1 pr-2">{formatCount(entry.search_queries)}</td>
									<td class="py-1"
										>{formatLatencyMs(entry.avg_search_latency_ms)}</td
									>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
		</section>
	</div>

	<!-- Operations -->
	<section
		class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
	>
		<div class="flex items-center justify-between mb-3">
			<h2 class="font-semibold text-gray-800 dark:text-gray-100">
				{t('monitoring.operations')}
			</h2>
			<button
				class="px-3 py-1 text-xs rounded bg-blue-500 hover:bg-blue-600 text-white disabled:opacity-50 cursor-pointer"
				onclick={loadOps}
				disabled={opsLoading}
			>
				{opsLoading ? t('monitoring.loading') : t('common.refresh')}
			</button>
		</div>
		{#if opsError}
			<p class="text-xs text-red-500 mb-2">{opsError}</p>
		{/if}
		<div class="grid md:grid-cols-2 gap-4">
			<div>
				<h3 class="text-xs font-medium text-gray-500 dark:text-gray-400 mb-1">
					{t('monitoring.activeTransactions')} ({activeTransactions.length})
				</h3>
				{#if activeTransactions.length === 0}
					<p class="text-sm text-gray-400">{t('monitoring.noData')}</p>
				{:else}
					<div class="overflow-x-auto">
						<table class="min-w-full text-xs font-mono">
							<thead>
								<tr class="text-left text-gray-500 dark:text-gray-400">
									<th class="py-1 pr-3">id</th>
									<th class="py-1 pr-3">{t('common.status')}</th>
									<th class="py-1 pr-3">{t('monitoring.duration')}</th>
									<th class="py-1 pr-3">{t('common.actions')}</th>
								</tr>
							</thead>
							<tbody>
								{#each activeTransactions as txn (txn.transaction_id)}
									<tr class="border-t border-gray-100 dark:border-gray-700/50">
										<td class="py-1 pr-3">{txn.transaction_id}</td>
										<td class="py-1 pr-3">{txn.state}</td>
										<td class="py-1 pr-3">{formatLatencyMs(txn.elapsed_ms)}</td>
										<td class="py-1 pr-3">
											<button
												class="text-red-500 hover:text-red-700 cursor-pointer"
												onclick={() => killTransaction(txn.transaction_id)}
											>
												{t('monitoring.kill')}
											</button>
										</td>
									</tr>
								{/each}
							</tbody>
						</table>
					</div>
				{/if}
			</div>
			<div>
				<h3 class="text-xs font-medium text-gray-500 dark:text-gray-400 mb-1">
					{t('monitoring.config')}
				</h3>
				{#if configMessage}
					<p class="text-xs text-green-600 dark:text-green-400 mb-2">{configMessage}</p>
				{/if}
				{#if Object.keys(configSections).length > 0}
					<div class="space-y-3 max-h-96 overflow-auto">
						{#each Object.entries(configSections) as [section, values] (section)}
							<details
								class="border border-gray-200 dark:border-gray-700 rounded"
								open={section === 'monitoring'}
							>
								<summary
									class="px-3 py-1.5 text-xs font-medium bg-gray-50 dark:bg-gray-800/50 cursor-pointer text-gray-700 dark:text-gray-300"
								>
									{section}
								</summary>
								<div class="p-2 space-y-2">
									{#each Object.keys(values) as key (key)}
										{@const entry = configEntryKey(section, key)}
										<div class="flex items-center gap-2 text-xs">
											<span class="font-mono w-40 shrink-0 truncate" title={key}>{key}</span>
											<input
												class="flex-1 px-2 py-1 border border-gray-300 dark:border-gray-600 rounded font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
												bind:value={configDrafts[entry]}
											/>
											<button
												class="px-2 py-1 rounded bg-blue-500 hover:bg-blue-600 text-white disabled:opacity-50 cursor-pointer"
												onclick={() => saveConfigKey(section, key)}
												disabled={configBusy[entry] === true}
											>
												{t('common.save')}
											</button>
											<button
												class="px-2 py-1 rounded bg-gray-100 dark:bg-gray-700/50 text-gray-700 dark:text-gray-200 hover:bg-gray-200 dark:hover:bg-gray-700 disabled:opacity-50 cursor-pointer"
												onclick={() => resetConfigKey(section, key)}
												disabled={configBusy[entry] === true}
											>
												{t('common.clear')}
											</button>
										</div>
									{/each}
								</div>
							</details>
						{/each}
					</div>
				{:else if configText}
					<pre
						class="text-xs font-mono bg-gray-50 dark:bg-gray-800/50 p-3 rounded border border-gray-200 dark:border-gray-700 overflow-auto max-h-64 text-gray-700 dark:text-gray-300">{configText}</pre
					>
				{:else}
					<p class="text-sm text-gray-400">{t('monitoring.configHint')}</p>
				{/if}
			</div>
		</div>
	</section>

	<!-- Portrait drawer -->
	{#if selectedTrace}
		<div class="fixed inset-0 z-50 flex justify-end">
			<button
				class="absolute inset-0 bg-black/30 cursor-pointer"
				onclick={closePortrait}
				aria-label={t('common.close')}
			></button>
			<div
				class="relative w-full max-w-md h-full bg-white dark:bg-[#1C2333] shadow-xl border-l border-gray-200 dark:border-gray-700 p-5 overflow-y-auto"
			>
				<div class="flex items-center justify-between mb-3">
					<h2 class="font-semibold text-gray-800 dark:text-gray-100">
						{t('monitoring.portrait')}
					</h2>
					<button
						class="px-2 py-1 text-sm text-gray-500 hover:text-gray-800 dark:hover:text-gray-200 cursor-pointer"
						onclick={closePortrait}
					>
						{t('common.close')}
					</button>
				</div>
				{#if portraitLoading}
					<p class="text-sm text-gray-500">{t('monitoring.loading')}</p>
				{:else if portraitError}
					<p class="text-sm text-red-500">{portraitError}</p>
				{:else if portrait}
					<div class="space-y-3 text-sm">
						<div
							class="font-mono text-xs break-all bg-gray-50 dark:bg-gray-800/50 rounded p-2"
						>
							{portrait.trace_id}
						</div>
						<div class="flex gap-2">
							<button
								class="px-2 py-1 text-xs bg-gray-100 dark:bg-gray-700 rounded cursor-pointer"
								onclick={copyTrace}
							>
								{copied ? t('monitoring.copied') : t('monitoring.copyTrace')}
							</button>
							<button
								class="px-2 py-1 text-xs bg-blue-500 text-white rounded cursor-pointer"
								onclick={backToConsole}
							>
								{t('monitoring.backToConsole')}
							</button>
						</div>
						<div
							class="font-mono text-xs break-words bg-gray-50 dark:bg-gray-800/50 rounded p-2"
						>
							{portrait.query}
						</div>
						<div class="grid grid-cols-2 gap-2 font-mono">
							<div>
								<span class="text-gray-500"
									>{t('monitoring.duration')}:
								</span>{formatLatencyMs(portrait.duration_ms)}
							</div>
							<div>
								<span class="text-gray-500">status: </span>{portrait.status}
							</div>
							<div>
								<span class="text-gray-500"
									>{t('monitoring.resultCount')}:
								</span>{formatCount(portrait.result_count)}
							</div>
							<div>
								<span class="text-gray-500"
									>{t('monitoring.planNodes')}:
								</span>{formatCount(portrait.plan_node_count)}
							</div>
						</div>
						{#if portrait.stages}
							<div>
								<h3 class="text-xs font-medium text-gray-500 mb-1">
									{t('monitoring.stages')}
								</h3>
								<ul class="font-mono space-y-1">
									<li class="flex justify-between">
										<span>{t('monitoring.stageParse')}</span><span
											>{formatLatencyMs(portrait.stages.parse_ms)}</span
										>
									</li>
									<li class="flex justify-between">
										<span>{t('monitoring.stageValidate')}</span><span
											>{formatLatencyMs(portrait.stages.validate_ms)}</span
										>
									</li>
									<li class="flex justify-between">
										<span>{t('monitoring.stagePlan')}</span><span
											>{formatLatencyMs(portrait.stages.plan_ms)}</span
										>
									</li>
									<li class="flex justify-between">
										<span>{t('monitoring.stageOptimize')}</span><span
											>{formatLatencyMs(portrait.stages.optimize_ms)}</span
										>
									</li>
									<li class="flex justify-between">
										<span>{t('monitoring.stageExecute')}</span><span
											>{formatLatencyMs(portrait.stages.execute_ms)}</span
										>
									</li>
								</ul>
							</div>
						{/if}
						{#if portrait.executors && portrait.executors.length > 0}
							<div>
								<h3 class="text-xs font-medium text-gray-500 mb-1">
									{t('monitoring.stageExecutors')}
								</h3>
								<ul class="font-mono space-y-1">
									{#each portrait.executors as ex (ex.executor_type)}
										<li class="flex justify-between">
											<span>{ex.executor_type}</span>
											<span
												>{formatLatencyMs(ex.duration_ms)} / {formatCount(
													ex.rows,
												)}</span
											>
										</li>
									{/each}
								</ul>
							</div>
						{/if}
						{#if portrait.error}
							<div>
								<h3 class="text-xs font-medium text-gray-500 mb-1">
									{t('monitoring.errorDetail')}
								</h3>
								<p
									class="font-mono text-xs text-red-600 dark:text-red-400 break-words bg-red-50 dark:bg-red-900/20 rounded p-2"
								>
									{portrait.error}
								</p>
							</div>
						{/if}
					</div>
				{/if}
			</div>
		</div>
	{/if}
</div>
