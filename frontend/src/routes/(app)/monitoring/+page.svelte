<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { t } from '$i18n';
	import {
		monitoringStore,
		type MonitoringSnapshots,
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

	const overview = $derived(monitor.snapshots.overview);
	const system = $derived(monitor.snapshots.system ?? overview?.system ?? null);
	const database = $derived(
		monitor.snapshots.database ?? overview?.database ?? null,
	);
	const queries = $derived(monitor.snapshots.queries);
	const search = $derived(monitor.snapshots.search);
	const retrieval = $derived(monitor.snapshots.retrieval);

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
		monitoringStore.setPaused(!monitor.paused);
	}

	interface Thresholds {
		cpuPercent: number;
		memPercent: number;
		slowMs: number;
	}

	function loadThresholds(): Thresholds {
		const fallback: Thresholds = {
			cpuPercent: 80,
			memPercent: 80,
			slowMs: 500,
		};
		try {
			const raw = localStorage.getItem('linkrs_monitor_thresholds');
			if (!raw) return fallback;
			const parsed = JSON.parse(raw) as Partial<Thresholds>;
			return {
				cpuPercent:
					typeof parsed.cpuPercent === 'number'
						? parsed.cpuPercent
						: fallback.cpuPercent,
				memPercent:
					typeof parsed.memPercent === 'number'
						? parsed.memPercent
						: fallback.memPercent,
				slowMs:
					typeof parsed.slowMs === 'number' ? parsed.slowMs : fallback.slowMs,
			};
		} catch {
			return fallback;
		}
	}

	let thresholds = $state<Thresholds>({
		cpuPercent: 80,
		memPercent: 80,
		slowMs: 500,
	});
	let thresholdsReady = $state(false);

	$effect(() => {
		if (!thresholdsReady) return;
		try {
			localStorage.setItem(
				'linkrs_monitor_thresholds',
				JSON.stringify(thresholds),
			);
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

<div class="ui-page-doc animate-fade-in">
	<PageHeader title={t('sidebar.monitoring')}>
		{#if monitor.lastRefreshAt}
			<span class="text-sm text-muted">
				{t('monitoring.lastRefresh')}: {new Date(
					monitor.lastRefreshAt,
				).toLocaleTimeString()}
			</span>
		{/if}
		<button class="ui-btn ui-btn-soft ui-btn-sm" onclick={togglePaused}>
			{monitor.paused ? t('monitoring.resume') : t('monitoring.pause')}
		</button>
		<button
			class="ui-btn ui-btn-primary ui-btn-sm"
			onclick={() => monitoringStore.refresh()}
			disabled={monitor.loading}
		>
			{monitor.loading ? t('monitoring.loading') : t('common.refresh')}
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
				type="number"
				min="1"
				max="100"
				class="w-16 ui-input ui-input-sm font-mono"
				bind:value={thresholds.cpuPercent}
			/>%
		</label>
		<label
			class="flex items-center gap-1 text-xs text-muted"
		>
			{t('monitoring.memory')} &gt;
			<input
				type="number"
				min="1"
				max="100"
				class="w-16 ui-input ui-input-sm font-mono"
				bind:value={thresholds.memPercent}
			/>%
		</label>
		<label
			class="flex items-center gap-1 text-xs text-muted"
		>
			{t('monitoring.slowQueries')} &gt;
			<input
				type="number"
				min="1"
				class="w-20 ui-input ui-input-sm font-mono"
				bind:value={thresholds.slowMs}
			/>ms
		</label>
		{#if cpuExceeded || memExceeded}
			<span class="text-xs font-medium text-error">
				{t('monitoring.thresholdExceeded')}
			</span>
		{/if}
	</section>

	{#if monitor.error}
		<div class="ui-alert ui-alert-warn">
			{t('monitoring.loadFailed')}: {monitor.error}
		</div>
	{/if}

	<!-- Overview -->
	<OverviewCards
		overview={overview}
		system={system}
		database={database}
		queries={queries}
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

	<h3 class="text-xs font-medium text-muted mb-1">
		{t('monitoring.slowQueries')}
	</h3>
	<SlowQueryTable
		rows={slowRows}
		slowMs={thresholds.slowMs}
		onselect={(traceId) => (selectedTrace = traceId)}
	/>

	<!-- Transactions + sync summary + search -->
	<div class="grid md:grid-cols-3 gap-4">
		<TransactionPanel
			overview={overview}
			transactionSnapshot={monitor.snapshots.transaction}
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

	<!-- Portrait drawer -->
	{#if selectedTrace}
		<PortraitDrawer
			traceId={selectedTrace}
			onclose={() => (selectedTrace = null)}
		/>
	{/if}
</div>
