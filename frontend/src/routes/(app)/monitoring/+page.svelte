<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { get, fromStore } from 'svelte/store';
	import { t } from '$i18n';
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
		cpuRatio !== null && cpuRatio * 100 > thresholds.cpuPercent,
	);
	const memExceeded = $derived(
		memRatio !== null && memRatio * 100 > thresholds.memPercent,
	);

	onMount(() => {
		monitoringStore.startPolling();
		return () => monitoringStore.stopPolling();
	});

	onDestroy(() => monitoringStore.stopPolling());
</script>

<div class="ui-page-doc animate-fade-in">
	<PageHeader title={t('sidebar.monitoring')}>
		{#if monitorState.current.lastRefreshAt}
			<span class="text-sm text-muted">
				{t('monitoring.lastRefresh')}: {new Date(
					monitorState.current.lastRefreshAt,
				).toLocaleTimeString()}
			</span>
		{/if}
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
		slowMs={thresholds.slowMs}
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

	<!-- Portrait drawer -->
	{#if selectedTrace}
		<PortraitDrawer
			traceId={selectedTrace}
			onclose={() => (selectedTrace = null)}
		/>
	{/if}
</div>
