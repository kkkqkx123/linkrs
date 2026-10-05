<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { t } from '$i18n';
	import { navigate } from 'svelte-routing';
	import { monitoringStore, type MonitoringSnapshots } from '$stores/monitoring';
	import { consoleStore } from '$stores/console';
	import { statisticsService, type QueryProfileDetailResponse } from '$services/statistics';
	import {
		formatBytes,
		formatCount,
		formatLatencyMs,
		formatLatencyUs,
		formatPercent,
		formatPermille,
		formatQps,
		formatUptimeSecs
	} from '$utils/metricsFormat';

	let monitor = $state({
		snapshots: {
			overview: null,
			system: null,
			database: null,
			queries: null,
			search: null,
			transaction: null,
			sync: null
		} as MonitoringSnapshots,
		lastRefreshAt: null as number | null,
		loading: false,
		paused: false,
		error: null as string | null
	});

	let selectedTrace = $state<string | null>(null);
	let portrait = $state<QueryProfileDetailResponse | null>(null);
	let portraitLoading = $state(false);
	let portraitError = $state<string | null>(null);
	let copied = $state(false);

	const overview = $derived(monitor.snapshots.overview);
	const system = $derived(monitor.snapshots.system ?? overview?.system ?? null);
	const database = $derived(monitor.snapshots.database ?? overview?.database ?? null);
	const queries = $derived(monitor.snapshots.queries);
	const search = $derived(monitor.snapshots.search);

	const trendPoints = $derived(overview?.timeseries ?? []);
	const trendPath = $derived.by(() => {
		if (trendPoints.length === 0) return '';
		const width = 600;
		const height = 80;
		const maxQ = Math.max(1, ...trendPoints.map((p) => Number(p.queries) || 0));
		const step = trendPoints.length > 1 ? width / (trendPoints.length - 1) : 0;
		return trendPoints
			.map((p, i) => {
				const x = (i * step).toFixed(1);
				const y = (height - ((Number(p.queries) || 0) / maxQ) * (height - 8) - 4).toFixed(1);
				return `${i === 0 ? 'M' : 'L'}${x},${y}`;
			})
			.join(' ');
	});

	const memRatio = $derived.by(() => {
		const used = Number(system?.memory_usage?.used_bytes ?? NaN);
		const total = Number(system?.memory_usage?.total_bytes ?? NaN);
		if (!Number.isFinite(used) || !Number.isFinite(total) || total <= 0) return null;
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
			portraitError = err instanceof Error ? err.message : 'Request failed';
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
		navigate('/console');
	}

	function togglePaused() {
		monitoringStore.setPaused(!monitor.paused);
	}

	onMount(() => {
		const unsub = monitoringStore.subscribe((s) => {
			monitor = {
				snapshots: s.snapshots as MonitoringSnapshots,
				lastRefreshAt: s.lastRefreshAt,
				loading: s.loading,
				paused: s.paused,
				error: s.error
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
		<h1 class="text-xl font-bold text-gray-800 dark:text-gray-100">{$t('sidebar.monitoring')}</h1>
		<div class="flex items-center gap-2 text-sm">
			{#if monitor.lastRefreshAt}
				<span class="text-gray-500 dark:text-gray-400">
					{$t('monitoring.lastRefresh')}: {new Date(monitor.lastRefreshAt).toLocaleTimeString()}
				</span>
			{/if}
			<button
				class="px-3 py-1 rounded bg-gray-100 dark:bg-gray-700/50 text-gray-700 dark:text-gray-200 hover:bg-gray-200 dark:hover:bg-gray-700 cursor-pointer"
				onclick={togglePaused}
			>
				{monitor.paused ? $t('monitoring.resume') : $t('monitoring.pause')}
			</button>
			<button
				class="px-3 py-1 rounded bg-blue-500 hover:bg-blue-600 text-white disabled:opacity-50 cursor-pointer"
				onclick={() => monitoringStore.refresh()}
				disabled={monitor.loading}
			>
				{monitor.loading ? $t('monitoring.loading') : $t('common.refresh')}
			</button>
		</div>
	</div>

	{#if monitor.error}
		<div
			class="p-2 bg-yellow-50 dark:bg-yellow-900/20 border border-yellow-200 dark:border-yellow-800 rounded text-xs text-yellow-700 dark:text-yellow-300"
		>
			{$t('monitoring.loadFailed')}: {monitor.error}
		</div>
	{/if}

	<!-- Overview -->
	<section
		class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
	>
		<h2 class="font-semibold text-gray-800 dark:text-gray-100 mb-3">{$t('monitoring.overview')}</h2>
		<div class="grid grid-cols-2 md:grid-cols-4 gap-3 text-sm">
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('common.status')}</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{system ? 'healthy' : $t('monitoring.noData')}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.connections')}</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatCount(system?.connections?.active)} / {formatCount(system?.connections?.max)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.uptime')}</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatUptimeSecs(system?.uptime_secs)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.qps')}</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatQps(database?.performance?.queries_per_second)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.avgLatency')}</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatLatencyMs(database?.performance?.avg_latency_ms)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">p50 / p95 / p99</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatLatencyUs(overview?.query_latency_us?.p50)} /
					{formatLatencyUs(overview?.query_latency_us?.p95)} /
					{formatLatencyUs(overview?.query_latency_us?.p99)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.errors')}</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatCount(overview?.errors?.total ?? database?.performance?.error_total)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.totalQueries')}</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatCount(database?.performance?.total_queries ?? queries?.total_queries)}
				</div>
			</div>
		</div>
		{#if trendPoints.length > 0}
			<div class="mt-4">
				<div class="text-xs text-gray-500 dark:text-gray-400 mb-1">
					{$t('monitoring.trend')}: {formatCount(
						trendPoints.reduce((a, p) => a + (Number(p.queries) || 0), 0)
					)}
					{$t('monitoring.queriesAvg')}
					{formatLatencyMs(
						trendPoints.reduce((a, p) => a + (Number(p.avg_latency_ms) || 0), 0) /
							Math.max(1, trendPoints.length)
					)}
				</div>
				<svg
					viewBox="0 0 600 80"
					class="w-full h-20 bg-gray-50 dark:bg-gray-800/50 rounded"
					preserveAspectRatio="none"
				>
					<path d={trendPath} fill="none" stroke="#3b82f6" stroke-width="2" />
				</svg>
			</div>
		{/if}
	</section>

	<!-- Resources -->
	<section
		class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
	>
		<h2 class="font-semibold text-gray-800 dark:text-gray-100 mb-3">{$t('monitoring.resources')}</h2>
		<div class="grid grid-cols-2 md:grid-cols-4 gap-3 text-sm">
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.cpu')}</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{cpuRatio !== null ? formatPercent(cpuRatio) : '—'}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.memory')}</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatBytes(system?.memory_usage?.used_bytes)} / {formatBytes(
						system?.memory_usage?.total_bytes
					)}
					{#if memRatio !== null}({formatPercent(memRatio)}){/if}
				</div>
				{#if memRatio !== null}
					<div class="h-1.5 mt-1 bg-gray-100 dark:bg-gray-700 rounded overflow-hidden">
						<div class="h-full bg-blue-500" style="width: {Math.min(100, memRatio * 100)}%"></div>
					</div>
				{/if}
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('common.processMemory')}</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatBytes(system?.process_memory_bytes)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.dataDir')}</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatBytes(system?.data_dir_size_bytes)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.walDir')}</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatBytes(system?.wal_dir_size_bytes)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.maxFds')}</div>
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
		<h2 class="font-semibold text-gray-800 dark:text-gray-100 mb-3">{$t('monitoring.queries')}</h2>
		<div class="grid grid-cols-2 md:grid-cols-4 gap-3 text-sm mb-4">
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.totalQueries')}</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatCount(queries?.total_queries)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.active')}</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatCount(database?.performance?.active_queries)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.avgLatency')}</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatLatencyUs(queries?.latency_percentiles_us?.avg)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.errors')}</div>
				<div class="font-mono text-gray-800 dark:text-gray-200">
					{formatCount(queries?.error_total)}
				</div>
			</div>
		</div>

		{#if queries}
			<div class="grid md:grid-cols-2 gap-4 mb-4 text-sm">
				<div>
					<h3 class="text-xs font-medium text-gray-500 dark:text-gray-400 mb-1">
						{$t('monitoring.errorsByType')}
					</h3>
					{#if Object.keys(queries.errors_by_type ?? {}).length === 0}
						<p class="text-gray-400">{$t('monitoring.noData')}</p>
					{:else}
						<ul class="space-y-1">
							{#each Object.entries(queries.errors_by_type ?? {}) as [kind, count] (kind)}
								<li class="flex justify-between font-mono">
									<span class="text-gray-600 dark:text-gray-300">{kind}</span>
									<span class="text-gray-800 dark:text-gray-200">{formatCount(count)}</span>
								</li>
							{/each}
						</ul>
					{/if}
				</div>
				<div>
					<h3 class="text-xs font-medium text-gray-500 dark:text-gray-400 mb-1">
						{$t('monitoring.errorsByPhase')}
					</h3>
					{#if Object.keys(queries.errors_by_phase ?? {}).length === 0}
						<p class="text-gray-400">{$t('monitoring.noData')}</p>
					{:else}
						<ul class="space-y-1">
							{#each Object.entries(queries.errors_by_phase ?? {}) as [phase, count] (phase)}
								<li class="flex justify-between font-mono">
									<span class="text-gray-600 dark:text-gray-300">{phase}</span>
									<span class="text-gray-800 dark:text-gray-200">{formatCount(count)}</span>
								</li>
							{/each}
						</ul>
					{/if}
				</div>
			</div>
		{/if}

		<h3 class="text-xs font-medium text-gray-500 dark:text-gray-400 mb-1">
			{$t('monitoring.topPatterns')}
		</h3>
		<div class="overflow-x-auto mb-4">
			{#if patterns.length === 0}
				<p class="text-sm text-gray-400">{$t('monitoring.noData')}</p>
			{:else}
				<table class="min-w-full text-sm">
					<thead>
						<tr class="text-left text-xs text-gray-500 dark:text-gray-400">
							<th class="py-1 pr-3">{$t('monitoring.pattern')}</th>
							<th class="py-1 pr-3">{$t('monitoring.count')}</th>
							<th class="py-1 pr-3">{$t('monitoring.avg')}</th>
							<th class="py-1 pr-3">p95</th>
							<th class="py-1 pr-3">p99</th>
							<th class="py-1 pr-3">{$t('monitoring.errorRate')}</th>
						</tr>
					</thead>
					<tbody>
						{#each patterns as p (p.normalized_query)}
							<tr class="border-t border-gray-100 dark:border-gray-700/50 font-mono">
								<td class="py-1 pr-3 max-w-72 truncate text-gray-700 dark:text-gray-200">
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
			{$t('monitoring.executors')}
		</h3>
		<div class="overflow-x-auto mb-4">
			{#if executors.length === 0}
				<p class="text-sm text-gray-400">{$t('monitoring.noData')}</p>
			{:else}
				<table class="min-w-full text-sm">
					<thead>
						<tr class="text-left text-xs text-gray-500 dark:text-gray-400">
							<th class="py-1 pr-3">{$t('monitoring.executor')}</th>
							<th class="py-1 pr-3">{$t('monitoring.count')}</th>
							<th class="py-1 pr-3">{$t('monitoring.totalTime')}</th>
							<th class="py-1 pr-3">{$t('monitoring.totalRows')}</th>
						</tr>
					</thead>
					<tbody>
						{#each executors as e (e.executor_type)}
							<tr class="border-t border-gray-100 dark:border-gray-700/50 font-mono">
								<td class="py-1 pr-3 text-gray-700 dark:text-gray-200">{e.executor_type}</td>
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
			{$t('monitoring.slowQueries')}
		</h3>
		<div class="overflow-x-auto">
			{#if slowRows.length === 0}
				<p class="text-sm text-gray-400">{$t('monitoring.noData')}</p>
			{:else}
				<table class="min-w-full text-sm">
					<thead>
						<tr class="text-left text-xs text-gray-500 dark:text-gray-400">
							<th class="py-1 pr-3">{$t('monitoring.query')}</th>
							<th class="py-1 pr-3">{$t('monitoring.duration')}</th>
							<th class="py-1 pr-3">{$t('monitoring.trace')}</th>
						</tr>
					</thead>
					<tbody>
						{#each slowRows as row (row.trace_id)}
							<tr class="border-t border-gray-100 dark:border-gray-700/50">
								<td class="py-1 pr-3 max-w-80 truncate font-mono text-gray-700 dark:text-gray-200">
									<button
										class="hover:underline cursor-pointer text-left"
										onclick={() => openPortrait(row.trace_id)}
									>
										{row.query}
									</button>
								</td>
								<td class="py-1 pr-3 font-mono">{formatLatencyMs(row.duration_ms)}</td>
								<td class="py-1 pr-3 font-mono text-xs text-gray-500">{row.trace_id}</td>
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
		<h2 class="font-semibold text-gray-800 dark:text-gray-100 mb-3">{$t('monitoring.storage')}</h2>
		<div class="grid grid-cols-2 md:grid-cols-4 gap-3 text-sm">
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.spaces')}</div>
				<div class="font-mono">{formatCount(database?.spaces?.count)}</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.vertices')}</div>
				<div class="font-mono">{formatCount(database?.spaces?.total_vertices)}</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.edges')}</div>
				<div class="font-mono">{formatCount(database?.spaces?.total_edges)}</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.totalSize')}</div>
				<div class="font-mono">{formatBytes(database?.storage?.total_size_bytes)}</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.indexSize')}</div>
				<div class="font-mono">{formatBytes(database?.storage?.index_size_bytes)}</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.dataSize')}</div>
				<div class="font-mono">{formatBytes(database?.storage?.data_size_bytes)}</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.fragmentation')}</div>
				<div class="font-mono">
					{formatPermille(database?.storage?.fragmentation_permille)} ({formatBytes(
						database?.storage?.wasted_bytes
					)})
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.tombstones')}</div>
				<div class="font-mono">
					{formatCount(database?.storage?.tombstone_count)} ({formatBytes(
						database?.storage?.tombstone_memory_bytes
					)})
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">{$t('monitoring.dirtyPages')}</div>
				<div class="font-mono">
					{formatCount(database?.storage?.dirty_pages)} / {formatCount(
						database?.storage?.dirty_pages_total
					)}
				</div>
			</div>
			<div>
				<div class="text-gray-500 dark:text-gray-400 text-xs">
					{$t('monitoring.checkpointSuccess')}
				</div>
				<div class="font-mono">
					{formatCount(database?.storage?.checkpoint?.success_count)} / {formatCount(
						database?.storage?.checkpoint?.failure_count
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
				{$t('monitoring.transactions')}
			</h2>
			<div class="space-y-1 text-sm font-mono">
				<div class="flex justify-between">
					<span class="text-gray-500">{$t('monitoring.begun')}</span>
					<span>{formatCount(overview?.transaction?.begun ?? num(monitor.snapshots.transaction, 'begun'))}</span>
				</div>
				<div class="flex justify-between">
					<span class="text-gray-500">{$t('monitoring.committed')}</span>
					<span>{formatCount(overview?.transaction?.committed ?? num(monitor.snapshots.transaction, 'committed'))}</span>
				</div>
				<div class="flex justify-between">
					<span class="text-gray-500">{$t('monitoring.rolledBack')}</span>
					<span>{formatCount(overview?.transaction?.rolled_back ?? num(monitor.snapshots.transaction, 'rolled_back'))}</span>
				</div>
				<div class="flex justify-between">
					<span class="text-gray-500">{$t('monitoring.active')}</span>
					<span>{formatCount(overview?.transaction?.active ?? num(monitor.snapshots.transaction, 'active'))}</span>
				</div>
			</div>
		</section>
		<section
			class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
		>
			<h2 class="font-semibold text-gray-800 dark:text-gray-100 mb-3">{$t('monitoring.sync')}</h2>
			<div class="space-y-1 text-sm font-mono">
				<div class="flex justify-between">
					<span class="text-gray-500">{$t('common.status')}</span>
					<span>{overview?.sync?.is_running ?? num(monitor.snapshots.sync, 'is_running') ? 'running' : '—'}</span>
				</div>
				<div class="flex justify-between">
					<span class="text-gray-500">{$t('monitoring.outbox')}</span>
					<span>
						{formatCount(overview?.sync?.outbox_pending ?? num(monitor.snapshots.sync, 'outbox_pending'))}
						/
						{formatCount(overview?.sync?.outbox_retries ?? num(monitor.snapshots.sync, 'outbox_retries'))}
						/
						{formatCount(overview?.sync?.outbox_dead_lettered ?? num(monitor.snapshots.sync, 'outbox_dead_lettered'))}
					</span>
				</div>
			</div>
		</section>
		<section
			class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
		>
			<h2 class="font-semibold text-gray-800 dark:text-gray-100 mb-3">{$t('common.search')}</h2>
			<div class="space-y-1 text-sm font-mono">
				<div class="flex justify-between">
					<span class="text-gray-500">{$t('monitoring.totalQueries')}</span>
					<span>{formatCount(search?.search?.total_queries)}</span>
				</div>
				<div class="flex justify-between">
					<span class="text-gray-500">{$t('monitoring.avgLatency')}</span>
					<span>{formatLatencyMs(search?.search?.avg_latency_ms)}</span>
				</div>
				<div class="flex justify-between">
					<span class="text-gray-500">cache</span>
					<span>{formatPercent(search?.cache?.hit_rate)}</span>
				</div>
			</div>
			{#if byIndex.length > 0}
				<h3 class="text-xs font-medium text-gray-500 dark:text-gray-400 mt-3 mb-1">
					{$t('monitoring.byIndex')}
				</h3>
				<div class="overflow-x-auto">
					<table class="min-w-full text-xs font-mono">
						<tbody>
							{#each byIndex.slice(0, 8) as entry (entry.index)}
								<tr class="border-t border-gray-100 dark:border-gray-700/50">
									<td class="py-1 pr-2 max-w-40 truncate">{entry.index}</td>
									<td class="py-1 pr-2">{formatCount(entry.search_queries)}</td>
									<td class="py-1">{formatLatencyMs(entry.avg_search_latency_ms)}</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
		</section>
	</div>

	<!-- Portrait drawer -->
	{#if selectedTrace}
		<div class="fixed inset-0 z-50 flex justify-end">
			<button
				class="absolute inset-0 bg-black/30 cursor-pointer"
				onclick={closePortrait}
				aria-label={$t('common.close')}
			></button>
			<div
				class="relative w-full max-w-md h-full bg-white dark:bg-[#1C2333] shadow-xl border-l border-gray-200 dark:border-gray-700 p-5 overflow-y-auto"
			>
				<div class="flex items-center justify-between mb-3">
					<h2 class="font-semibold text-gray-800 dark:text-gray-100">
						{$t('monitoring.portrait')}
					</h2>
					<button
						class="px-2 py-1 text-sm text-gray-500 hover:text-gray-800 dark:hover:text-gray-200 cursor-pointer"
						onclick={closePortrait}
					>
						{$t('common.close')}
					</button>
				</div>
				{#if portraitLoading}
					<p class="text-sm text-gray-500">{$t('monitoring.loading')}</p>
				{:else if portraitError}
					<p class="text-sm text-red-500">{portraitError}</p>
				{:else if portrait}
					<div class="space-y-3 text-sm">
						<div class="font-mono text-xs break-all bg-gray-50 dark:bg-gray-800/50 rounded p-2">
							{portrait.trace_id}
						</div>
						<div class="flex gap-2">
							<button
								class="px-2 py-1 text-xs bg-gray-100 dark:bg-gray-700 rounded cursor-pointer"
								onclick={copyTrace}
							>
								{copied ? $t('monitoring.copied') : $t('monitoring.copyTrace')}
							</button>
							<button
								class="px-2 py-1 text-xs bg-blue-500 text-white rounded cursor-pointer"
								onclick={backToConsole}
							>
								{$t('monitoring.backToConsole')}
							</button>
						</div>
						<div class="font-mono text-xs break-words bg-gray-50 dark:bg-gray-800/50 rounded p-2">
							{portrait.query}
						</div>
						<div class="grid grid-cols-2 gap-2 font-mono">
							<div>
								<span class="text-gray-500">{$t('monitoring.duration')}: </span>{formatLatencyMs(
									portrait.duration_ms
								)}
							</div>
							<div>
								<span class="text-gray-500">status: </span>{portrait.status}
							</div>
							<div>
								<span class="text-gray-500">{$t('monitoring.resultCount')}: </span>{formatCount(
									portrait.result_count
								)}
							</div>
							<div>
								<span class="text-gray-500">{$t('monitoring.planNodes')}: </span>{formatCount(
									portrait.plan_node_count
								)}
							</div>
						</div>
						{#if portrait.stages}
							<div>
								<h3 class="text-xs font-medium text-gray-500 mb-1">{$t('monitoring.stages')}</h3>
								<ul class="font-mono space-y-1">
									<li class="flex justify-between">
										<span>parse</span><span>{formatLatencyMs(portrait.stages.parse_ms)}</span>
									</li>
									<li class="flex justify-between">
										<span>validate</span><span
											>{formatLatencyMs(portrait.stages.validate_ms)}</span
										>
									</li>
									<li class="flex justify-between">
										<span>plan</span><span>{formatLatencyMs(portrait.stages.plan_ms)}</span>
									</li>
									<li class="flex justify-between">
										<span>optimize</span><span
											>{formatLatencyMs(portrait.stages.optimize_ms)}</span
										>
									</li>
									<li class="flex justify-between">
										<span>execute</span><span
											>{formatLatencyMs(portrait.stages.execute_ms)}</span
										>
									</li>
								</ul>
							</div>
						{/if}
						{#if portrait.executors && portrait.executors.length > 0}
							<div>
								<h3 class="text-xs font-medium text-gray-500 mb-1">
									{$t('monitoring.stageExecutors')}
								</h3>
								<ul class="font-mono space-y-1">
									{#each portrait.executors as ex (ex.executor_type)}
										<li class="flex justify-between">
											<span>{ex.executor_type}</span>
											<span>{formatLatencyMs(ex.duration_ms)} / {formatCount(ex.rows)}</span>
										</li>
									{/each}
								</ul>
							</div>
						{/if}
						{#if portrait.error}
							<div>
								<h3 class="text-xs font-medium text-gray-500 mb-1">
									{$t('monitoring.errorDetail')}
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
