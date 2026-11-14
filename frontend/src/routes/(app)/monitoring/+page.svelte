<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { get, fromStore } from 'svelte/store';
	import { t } from '$i18n';
	import { appendAuditLog, loadAuditLog, type AuditEntry } from '$utils/auditLog';
	import {
		monitoringStore,
		type MonitoringSnapshots,
		type Thresholds,
	} from '$stores/monitoring';
	import PortraitDrawer from '$components/business/monitoring/PortraitDrawer.svelte';
	// Sync management moved to the dedicated /sync page.
	import OpsPanel from '$components/business/monitoring/OpsPanel.svelte';
	import OverviewCards from '$components/business/monitoring/OverviewCards.svelte';
	import SystemPanel from '$components/business/monitoring/SystemPanel.svelte';
	import QueryStatsPanel from '$components/business/monitoring/QueryStatsPanel.svelte';
	import SlowQueryTable from '$components/business/monitoring/SlowQueryTable.svelte';
	import RetrievalPanel from '$components/business/monitoring/RetrievalPanel.svelte';
	import TransactionPanel from '$components/business/monitoring/TransactionPanel.svelte';
	import SearchPanel from '$components/business/monitoring/SearchPanel.svelte';
	import PageHeader from '$components/layout/PageHeader.svelte';
	import { formatCount } from '$utils/metricsFormat';
	import {
		TREND_RANGES,
		type TrendRange,
	} from '$utils/timeWindow';

	const monitorState = fromStore(monitoringStore);
	const snapshots = $derived(
		monitorState.current.snapshots as MonitoringSnapshots,
	);

	let selectedTrace = $state<string | null>(null);

	const overview = $derived(snapshots.overview);
	const system = $derived(snapshots.system ?? overview?.system ?? null);
	const database = $derived(
		snapshots.database ?? overview?.database ?? null,
	);
	const queries = $derived(snapshots.queries);
	const search = $derived(snapshots.search);
	const retrieval = $derived(snapshots.retrieval);

	const slowRows = $derived(queries?.slow_queries ?? []);
	const patterns = $derived(queries?.top_patterns ?? []);
	const executors = $derived(queries?.executor_summary ?? []);
	const byIndex = $derived(search?.by_index ?? []);

	const memRatio = $derived.by(() => {
		const used = Number(system?.memory_usage?.used_bytes ?? NaN);
		const total = Number(system?.memory_usage?.total_bytes ?? NaN);
		if (!Number.isFinite(used) || !Number.isFinite(total) || total <= 0)
			return null;
		return used / total;
	});

	const cpuRatio = $derived.by(() => {
		const raw = Number(system?.cpu_usage_percent ?? NaN);
		return Number.isFinite(raw) ? raw / 100 : null;
	});

	function togglePaused() {
		monitoringStore.setPaused(!monitorState.current.paused);
	}

	// Editable thresholds live in the monitoring store (with persistence);
	// the local copy exists only so the inputs can bind, and every change is
	// written back through the store setter.
	let thresholds = $state<Thresholds>({ ...get(monitoringStore).thresholds });

	$effect(() => {
		monitoringStore.setThresholds({ ...thresholds });
	});

	const cpuExceeded = $derived(
		thresholds.cpuEnabled !== false &&
			cpuRatio !== null &&
			cpuRatio * 100 > thresholds.cpuPercent,
	);
	const memExceeded = $derived(
		thresholds.memEnabled !== false &&
			memRatio !== null &&
			memRatio * 100 > thresholds.memPercent,
	);

	// Readonly audit event stream: slow-query events come from the existing
	// query statistics snapshot; maintenance entries come from the local
	// operation log. No server persistence is assumed.
	let auditLog = $state<AuditEntry[]>(loadAuditLog());

	$effect(() => {
		// Reload the local maintenance log whenever a refresh lands so
		// records written by sibling panels appear without new fetching.
		void monitorState.current.lastRefreshAt;
		auditLog = loadAuditLog();
	});

	const auditEvents = $derived.by(() => {
		const slow = slowRows.slice(0, 20).map((row) => ({
			at: monitorState.current.lastRefreshAt ?? Date.now(),
			kind: 'slow-query',
			summary: `${String(row.query)} (${String(row.duration_ms)}ms)`,
			traceId: String(row.trace_id),
		}));
		const local = auditLog.map((entry) => ({
			at: entry.at,
			kind: entry.kind,
			summary: entry.summary,
			traceId: null as string | null,
		}));
		return [...slow, ...local].sort((a, b) => b.at - a.at).slice(0, 50);
	});

	function logThreshold(kind: string, summary: string) {
		auditLog = appendAuditLog(kind, summary);
	}

	onMount(() => {
		monitoringStore.start();
		return () => monitoringStore.stop();
	});

	onDestroy(() => monitoringStore.stop());
</script>

<div class="ui-page-doc animate-fade-in">
	<PageHeader title={t('sidebar.monitoring')}>
		{#if monitorState.current.streamStatus === 'live'}
			<span class="flex items-center gap-1.5 text-xs font-medium text-success">
				<span class="relative flex h-2 w-2">
					<span
						class="absolute inline-flex h-full w-full animate-ping rounded-full bg-success opacity-60"
					></span>
					<span class="relative inline-flex h-2 w-2 rounded-full bg-success"></span>
				</span>
				{t('monitoring.streamLive')}
			</span>
		{:else if monitorState.current.streamStatus === 'connecting'}
			<span class="text-xs text-muted">{t('monitoring.streamConnecting')}</span>
		{:else if monitorState.current.streamStatus === 'unavailable'}
			<span class="text-xs text-warning" title={t('monitoring.streamUnavailableHint')}>
				{t('monitoring.streamUnavailable')}
			</span>
		{/if}
		{#if monitorState.current.lastRefreshAt}
			<span class="text-sm text-muted">
				{t('monitoring.lastRefresh')}: {new Date(
					monitorState.current.lastRefreshAt,
				).toLocaleTimeString()}
			</span>
		{/if}
		<label class="flex items-center gap-1 text-sm text-muted">
			{t('monitoring.range')}
			<select
				class="ui-input ui-input-sm"
				value={monitorState.current.timeRange}
				onchange={(event) =>
					monitoringStore.setTimeRange(
						event.currentTarget.value as TrendRange,
					)}
			>
				{#each TREND_RANGES as range (range)}
					<option value={range}>{t(`monitoring.range_${range}`)}</option>
				{/each}
			</select>
		</label>
		<button class="ui-btn ui-btn-soft ui-btn-sm" onclick={togglePaused}>
			{monitorState.current.paused ? t('monitoring.resume') : t('monitoring.pause')}
		</button>
		<button
			class="ui-btn ui-btn-primary ui-btn-sm"
			onclick={() => monitoringStore.refresh()}
			disabled={monitorState.current.loading}
		>
			{monitorState.current.loading ? t('monitoring.loading') : t('common.refresh')}
		</button>
	</PageHeader>

	<section
		class="ui-card shadow-sm px-6 py-4 flex items-center gap-4 flex-wrap text-sm"
	>
		<span class="font-medium text-ink"
			>{t('monitoring.thresholds')}</span
		>
		<label
			class="flex items-center gap-1 text-xs text-muted"
		>
			{t('monitoring.cpu')} &gt;
			<input
				type="checkbox"
				bind:checked={thresholds.cpuEnabled}
				onchange={() =>
					logThreshold(
						'threshold',
						`cpu ${thresholds.cpuEnabled ? 'enabled' : 'disabled'} > ${thresholds.cpuPercent}%`,
					)}
			/>
			<input
				type="number"
				min="1"
				max="100"
				class="w-16 ui-input ui-input-sm font-mono"
				bind:value={thresholds.cpuPercent}
				onchange={() => logThreshold('threshold', `cpu > ${thresholds.cpuPercent}%`)}
			/>%
		</label>
		<label
			class="flex items-center gap-1 text-xs text-muted"
		>
			{t('monitoring.memory')} &gt;
			<input
				type="checkbox"
				bind:checked={thresholds.memEnabled}
				onchange={() =>
					logThreshold(
						'threshold',
						`memory ${thresholds.memEnabled ? 'enabled' : 'disabled'} > ${thresholds.memPercent}%`,
					)}
			/>
			<input
				type="number"
				min="1"
				max="100"
				class="w-16 ui-input ui-input-sm font-mono"
				bind:value={thresholds.memPercent}
				onchange={() => logThreshold('threshold', `memory > ${thresholds.memPercent}%`)}
			/>%
		</label>
		<label
			class="flex items-center gap-1 text-xs text-muted"
		>
			{t('monitoring.slowQueries')} &gt;
			<input
				type="checkbox"
				bind:checked={thresholds.slowEnabled}
				onchange={() =>
					logThreshold(
						'threshold',
						`slow queries ${thresholds.slowEnabled ? 'enabled' : 'disabled'} > ${thresholds.slowMs}ms`,
					)}
			/>
			<input
				type="number"
				min="1"
				class="w-20 ui-input ui-input-sm font-mono"
				bind:value={thresholds.slowMs}
				onchange={() => logThreshold('threshold', `slow queries > ${thresholds.slowMs}ms`)}
			/>ms
		</label>
		{#if cpuExceeded || memExceeded}
			<span class="text-xs font-medium text-error">
				{t('monitoring.thresholdExceeded')}
			</span>
		{/if}
	</section>

	{#if monitorState.current.error}
		<div class="ui-alert ui-alert-warn">
			{t('monitoring.loadFailed')}: {monitorState.current.error}
		</div>
	{/if}

	<!-- Overview -->
	<OverviewCards
		overview={overview}
		system={system}
		database={database}
		queries={queries}
		timeRange={monitorState.current.timeRange}
	/>

	<!-- Resources + Storage -->
	<SystemPanel
		system={system}
		database={database}
		cpuRatio={cpuRatio}
		memRatio={memRatio}
		cpuExceeded={cpuExceeded}
		memExceeded={memExceeded}
	/>

	<!-- Queries -->
	<QueryStatsPanel
		queries={queries}
		database={database}
		patterns={patterns}
		executors={executors}
	/>

	<h2 class="text-xs font-medium text-muted mb-1">
		{t('monitoring.slowQueries')}
	</h2>
	<SlowQueryTable
		rows={slowRows}
		slowMs={thresholds.slowEnabled === false ? Number.POSITIVE_INFINITY : thresholds.slowMs}
		onselect={(traceId) => (selectedTrace = traceId)}
	/>

	<!-- Transactions + sync summary + search -->
	<div class="grid md:grid-cols-3 gap-4">
		<TransactionPanel
			overview={overview}
			transactionSnapshot={snapshots.transaction}
		/>
		<section class="ui-card shadow-sm p-6">
			<div class="flex items-center justify-between mb-3">
				<h2 class="ui-section-title mb-0">
					{t('monitoring.sync')}
				</h2>
				<a class="ui-btn ui-btn-primary ui-btn-sm" href="/sync">
					{t('sidebar.sync')}
				</a>
			</div>
			<div class="space-y-1 text-sm font-mono">
				<div class="flex justify-between">
					<span class="text-muted">{t('common.status')}</span>
					<span>
						{overview?.sync?.is_running ? t('common.running') : '—'}
					</span>
				</div>
				<div class="flex justify-between">
					<span class="text-muted">{t('monitoring.outbox')}</span>
					<span>
						{formatCount(overview?.sync?.outbox_pending)} /
						{formatCount(overview?.sync?.outbox_retries)} /
						{formatCount(overview?.sync?.outbox_dead_lettered)}
					</span>
				</div>
			</div>
		</section>
		<SearchPanel search={search} byIndex={byIndex} />
	</div>

	<!-- Retrieval services -->
	<RetrievalPanel retrieval={retrieval} />

	<!-- Operations -->
	<OpsPanel />

	<!-- Audit: readonly event stream over query stats plus local maintenance -->
	<section class="ui-card shadow-sm p-6">
		<h2 class="ui-section-title">
			{t('monitoring.audit')}
		</h2>
		<p class="text-xs text-muted mb-3">{t('monitoring.auditHint')}</p>
		{#if auditEvents.length === 0}
			<p class="text-sm text-muted">{t('monitoring.noData')}</p>
		{:else}
			<div class="overflow-x-auto max-h-64 overflow-y-auto">
				<table class="ui-table w-full border-collapse">
					<thead>
						<tr class="bg-secondary">
							<th>{t('monitoring.auditTime')}</th>
							<th>{t('common.type')}</th>
							<th>{t('monitoring.query')}</th>
						</tr>
					</thead>
					<tbody>
						{#each auditEvents as event (event.traceId ?? `${event.kind}-${event.at}-${event.summary}`)}
							<tr>
								<td class="font-mono text-xs whitespace-nowrap">
									{new Date(event.at).toLocaleString()}
								</td>
								<td class="font-mono text-xs">{event.kind}</td>
								<td class="font-mono text-xs break-all">{event.summary}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</section>

	<!-- Portrait drawer -->
	{#if selectedTrace}
		<PortraitDrawer
			traceId={selectedTrace}
			onclose={() => (selectedTrace = null)}
		/>
	{/if}
</div>
