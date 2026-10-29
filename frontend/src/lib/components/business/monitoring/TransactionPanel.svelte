<script lang="ts">
	import { t } from '$i18n';
	import { formatCount } from '$utils/metricsFormat';
	import { operationsService } from '$services/operations';
	import type { ActiveTransaction } from '$services/operations';
	import type { OverviewResponse, TransactionMetrics } from '$services/statistics';

	let {
		overview,
		transactionSnapshot,
		transactions = null,
		onKill = null,
	}: {
		overview: OverviewResponse | null;
		transactionSnapshot: TransactionMetrics | null;
		transactions?: ActiveTransaction[] | null;
		onKill?: ((id: number) => Promise<void>) | null;
	} = $props();

	let internalTransactions = $state<ActiveTransaction[]>([]);
	let txLoading = $state(false);
	let txError = $state<string | null>(null);

	// The transactions page passes its own list to avoid a duplicate
	// `GET /v1/transactions`; other hosts (monitoring overview) let the
	// panel fetch on demand.
	const managedExternally = $derived(transactions !== null);
	const visibleTransactions = $derived(transactions ?? internalTransactions);

	async function loadActiveTransactions() {
		if (managedExternally) return;
		txLoading = true;
		txError = null;
		try {
			internalTransactions = await operationsService.transactions();
		} catch (err) {
			txError = err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			txLoading = false;
		}
	}

	async function killTransaction(id: number) {
		if (onKill) {
			await onKill(id);
			return;
		}
		try {
			await operationsService.killTransaction(id);
			await loadActiveTransactions();
		} catch (err) {
			txError = err instanceof Error ? err.message : t('notification.requestFailed');
		}
	}

	// Server `GET /v1/transactions/metrics` nests counters under `outcomes`
	// and gauges under `resources`; the overview flattens a subset.
	// Prefer overview, fall back to the outcomes branch — never read flat
	// keys off the metrics payload.
	function outcome(key: string): unknown {
		const flat = overview?.transaction as Record<string, unknown> | undefined;
		if (flat && flat[key] !== undefined) return flat[key];
		return transactionSnapshot?.outcomes?.[
			key as keyof NonNullable<TransactionMetrics['outcomes']>
		];
	}

	function resource(key: string): unknown {
		return transactionSnapshot?.resources?.[
			key as keyof NonNullable<TransactionMetrics['resources']>
		];
	}
	const outcomeRows = [
		{ label: t('monitoring.begun'), value: outcome('begun') },
		{ label: t('monitoring.committed'), value: outcome('committed') },
		{ label: t('monitoring.rolledBack'), value: outcome('rolled_back') ?? outcome('aborted') },
		{ label: t('monitoring.active'), value: outcome('active') },
		{ label: t('monitoring.conflict'), value: outcome('conflict') },
		{ label: t('monitoring.timeout'), value: outcome('timeout') },
	];

	const resourceRows = [
		{ label: t('monitoring.pendingWrites'), value: resource('pending_writes') },
		{ label: t('monitoring.stagedWalBytes'), value: resource('staged_wal_bytes') },
		{ label: t('monitoring.undoBytes'), value: resource('undo_bytes') },
		{ label: t('monitoring.frontierLag'), value: resource('committed_frontier_lag') },
		{ label: t('monitoring.activeSnapshots'), value: resource('active_snapshots') },
	];
</script>

<section class="ui-card shadow-sm p-6">
	<h2 class="ui-section-title">
		{t('monitoring.transactions')}
	</h2>
	<ul class="space-y-1 text-sm font-mono">
		{#each outcomeRows as row (row.label)}
			<li class="flex justify-between">
				<span class="text-muted">{row.label}</span>
				<span>{formatCount(row.value)}</span>
			</li>
		{/each}
	</ul>

	<h3 class="mt-4 mb-3 text-sm font-semibold">{t('monitoring.txResources')}</h3>
	<ul class="space-y-1 text-sm font-mono">
		{#each resourceRows as row (row.label)}
			<li class="flex justify-between">
				<span class="text-muted">{row.label}</span>
				<span>{formatCount(row.value)}</span>
			</li>
		{/each}
	</ul>

	<div class="mt-4 space-y-2">
		<div class="flex items-center justify-between">
			<h3 class="text-sm font-semibold">{t('monitoring.activeTransactions')}</h3>
			{#if !managedExternally}
				<button class="ui-btn ui-btn-secondary text-xs" onclick={loadActiveTransactions} disabled={txLoading}>
					{txLoading ? '...' : t('common.refresh')}
				</button>
			{/if}
		</div>
		{#if txError}
			<p class="text-xs text-red-500">{txError}</p>
		{/if}
		{#if visibleTransactions.length === 0}
			<p class="text-xs text-muted">{t('monitoring.noData')}</p>
		{:else}
			<ul class="space-y-1 text-xs font-mono">
				{#each visibleTransactions as tx (tx.transaction_id)}
					<li class="flex items-center justify-between gap-2">
						<span>#{tx.transaction_id} {tx.state} {tx.owner}</span>
						<button class="ui-btn ui-btn-danger text-xs" onclick={() => killTransaction(tx.transaction_id)}>
							{t('monitoring.kill')}
						</button>
					</li>
				{/each}
			</ul>
		{/if}
	</div>
</section>
