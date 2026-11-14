<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { t } from '$i18n';
	import { goto } from '$app/navigation';
	import { fromStore } from 'svelte/store';
	import {
		Keyboard,
		Database,
		Link2,
		Table,
		LineChart,
		ArrowRight,
	} from '@lucide/svelte';
	import { monitoringStore } from '$stores/monitoring';
	import {
		formatBytes,
		formatCount,
		formatLatencyMs,
		formatQps,
	} from '$utils/metricsFormat';

	const monitorState = fromStore(monitoringStore);
	// The dashboard consumes the shared monitoring snapshot instead of
	// fetching its own copy, so returning home never fires extra requests.
	const overview = $derived(monitorState.current.snapshots.database);
	const failed = $derived(
		overview === null && monitorState.current.error !== null,
	);

	onMount(() => {
		monitoringStore.start();
		return () => monitoringStore.stop();
	});

	onDestroy(() => monitoringStore.stop());
</script>

<div class="max-w-5xl mx-auto space-y-6 animate-fade-in">
	<!-- Live overview -->
	<div class="flex items-center justify-between">
		<h1 class="text-2xl font-bold text-ink">{t('app.title')}</h1>
		<button
			type="button"
			class="ui-btn ui-btn-ghost ui-btn-sm flex items-center gap-1.5"
			onclick={() => goto('/monitoring')}
		>
			{t('home.viewMonitoring')}
			<ArrowRight size={14} />
		</button>
	</div>

	{#if overview}
		<div class="grid grid-cols-2 md:grid-cols-4 gap-5">
			<!-- Data scale -->
			<div class="ui-card shadow-sm p-5">
				<p class="text-sm font-medium text-muted mb-1">
					{t('home.vertices')}
				</p>
				<p class="text-3xl font-bold text-ink">
					{formatCount(overview.spaces?.total_vertices)}
				</p>
				<p class="text-xs text-muted mt-2">
					{t('home.spaces')}: {formatCount(overview.spaces?.count)}
					· {t('home.edges')}:
					{formatCount(overview.spaces?.total_edges)}
				</p>
			</div>
			<!-- Storage -->
			<div class="ui-card shadow-sm p-5">
				<p class="text-sm font-medium text-muted mb-1">
					{t('home.storage')}
				</p>
				<p class="text-3xl font-bold text-ink">
					{formatBytes(overview.storage?.total_size_bytes)}
				</p>
				<p class="text-xs text-muted mt-2">
					{t('home.indexSize')}:
					{formatBytes(overview.storage?.index_size_bytes)}
				</p>
			</div>
			<!-- Query performance -->
			<div class="ui-card shadow-sm p-5">
				<p class="text-sm font-medium text-muted mb-1">
					{t('home.queries')}
				</p>
				<p class="text-3xl font-bold text-ink">
					{formatCount(overview.performance?.total_queries)}
				</p>
				<p class="text-xs text-muted mt-2">
					{t('home.avgLatency')}:
					{formatLatencyMs(overview.performance?.avg_latency_ms)}
					· {t('home.qps')}:
					{formatQps(overview.performance?.queries_per_second)}
				</p>
			</div>
			<!-- Search -->
			<div class="ui-card shadow-sm p-5">
				<p class="text-sm font-medium text-muted mb-1">
					{t('home.searchQueries')}
				</p>
				<p class="text-3xl font-bold text-ink">
					{formatCount(overview.search?.total_queries)}
				</p>
				<p class="text-xs text-muted mt-2">
					{t('home.searchErrors')}:
					{formatCount(overview.search?.total_errors)}
				</p>
			</div>
		</div>
	{:else if failed}
		<p class="ui-card shadow-sm p-5 text-sm text-muted">
			{t('home.unavailable')}
		</p>
	{:else}
		<div class="grid grid-cols-2 md:grid-cols-4 gap-5" aria-hidden="true">
			{#each [0, 1, 2, 3] as i (i)}
				<div class="ui-card shadow-sm p-5 animate-pulse">
					<div class="h-4 w-20 bg-secondary rounded mb-2"></div>
					<div class="h-8 w-24 bg-secondary rounded mb-3"></div>
					<div class="h-3 w-28 bg-secondary rounded"></div>
				</div>
			{/each}
		</div>
	{/if}

	<!-- Quick access -->
	<div class="ui-card shadow-sm p-8 animate-slide-up animation-delay-200">
		<h2 class="text-xl font-bold text-ink mb-4">
			{t('home.quickAccess')}
		</h2>
		<p class="text-muted mb-4 text-base">
			{t('mainPage.selectModule')}
		</p>
		<ul class="space-y-2.5 text-muted text-base">
			<li>
				<button
					class="w-full text-left flex items-center gap-2 rounded-md px-1 py-0.5 hover:bg-hover cursor-pointer transition-colors"
					onclick={() => goto('/console')}
				>
					<span class="text-info"><Keyboard size={18} /></span>
					<strong>{t('sidebar.console')}:</strong>
					{t('mainPage.consoleDesc')}
				</button>
			</li>
			<li>
				<button
					class="w-full text-left flex items-center gap-2 rounded-md px-1 py-0.5 hover:bg-hover cursor-pointer transition-colors"
					onclick={() => goto('/schema')}
				>
					<span class="text-info"><Database size={18} /></span>
					<strong>{t('sidebar.schema')}:</strong>
					{t('mainPage.schemaDesc')}
				</button>
			</li>
			<li>
				<button
					class="w-full text-left flex items-center gap-2 rounded-md px-1 py-0.5 hover:bg-hover cursor-pointer transition-colors"
					onclick={() => goto('/console')}
				>
					<span class="text-info"><Link2 size={18} /></span>
					<strong>{t('sidebar.graph')}:</strong>
					{t('mainPage.graphDesc')}
				</button>
			</li>
			<li>
				<button
					class="w-full text-left flex items-center gap-2 rounded-md px-1 py-0.5 hover:bg-hover cursor-pointer transition-colors"
					onclick={() => goto('/data-browser')}
				>
					<span class="text-info"><Table size={18} /></span>
					<strong>{t('sidebar.dataBrowser')}:</strong>
					{t('mainPage.dataBrowserDesc')}
				</button>
			</li>
			<li>
				<button
					class="w-full text-left flex items-center gap-2 rounded-md px-1 py-0.5 hover:bg-hover cursor-pointer transition-colors"
					onclick={() => goto('/monitoring')}
				>
					<span class="text-info"><LineChart size={18} /></span>
					<strong>{t('sidebar.monitoring')}:</strong>
					{t('mainPage.monitoringDesc')}
				</button>
			</li>
		</ul>
	</div>
</div>
