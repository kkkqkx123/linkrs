<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from '$i18n';
	import PageHeader from '$components/layout/PageHeader.svelte';
	import SyncPanel from '$components/business/monitoring/SyncPanel.svelte';
	import { statisticsService, type OverviewResponse } from '$services/statistics';
	import type { SyncStatusResponse } from '$services/sync';

	let syncPanel: SyncPanel | null = null;

	let overview = $state<OverviewResponse | null>(null);
	let syncSnapshot = $state<SyncStatusResponse | null>(null);
	let loading = $state(false);
	let error = $state<string | null>(null);

	async function load() {
		loading = true;
		error = null;
		try {
			const [ov, sync] = await Promise.all([
				statisticsService.overview(),
				statisticsService.syncStatus(),
			]);
			overview = ov;
			syncSnapshot = sync;
			await syncPanel?.reload();
		} catch (err) {
			error =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			loading = false;
		}
	}

	onMount(load);
</script>

<div class="ui-page-doc animate-fade-in">
	<PageHeader title={t('sidebar.sync')}>
		<button
			class="ui-btn ui-btn-primary ui-btn-sm"
			onclick={load}
			disabled={loading}
		>
			{loading ? t('monitoring.loading') : t('common.refresh')}
		</button>
	</PageHeader>

	{#if error}
		<p class="ui-error-text">{error}</p>
	{/if}

	<SyncPanel bind:this={syncPanel} overview={overview} syncSnapshot={syncSnapshot} />
</div>
