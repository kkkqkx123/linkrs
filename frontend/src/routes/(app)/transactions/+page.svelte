<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from '$i18n';
	import PageHeader from '$components/layout/PageHeader.svelte';
	import TransactionPanel from '$components/business/monitoring/TransactionPanel.svelte';
	import { statisticsService, type OverviewResponse, type TransactionMetrics } from '$services/statistics';
	import { operationsService, type ActiveTransaction } from '$services/operations';
	import { formatLatencyMs } from '$utils/metricsFormat';

	let overview = $state<OverviewResponse | null>(null);
	let transactionSnapshot = $state<TransactionMetrics | null>(null);
	let activeTransactions = $state<ActiveTransaction[]>([]);
	let loading = $state(false);
	let error = $state<string | null>(null);

	async function load() {
		loading = true;
		error = null;
		try {
			const [ov, metrics, txns] = await Promise.all([
				statisticsService.overview(),
				statisticsService.transactionMetrics(),
				operationsService.transactions(),
			]);
			overview = ov;
			transactionSnapshot = metrics;
			activeTransactions = txns;
		} catch (err) {
			error =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			loading = false;
		}
	}

	async function killTransaction(id: number) {
		if (!confirm(t('monitoring.confirmKill', { id: String(id) }))) return;
		loading = true;
		error = null;
		try {
			await operationsService.killTransaction(id);
			await load();
		} catch (err) {
			error =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			loading = false;
		}
	}

	async function retryTransactionOutbox(id: number) {
		if (!confirm(t('monitoring.confirmRetryTxn', { id: String(id) }))) return;
		loading = true;
		error = null;
		try {
			await operationsService.retryTransactionOutbox(id);
			await load();
		} catch (err) {
			error =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			loading = false;
		}
	}

	function hasBlockingReason(tx: ActiveTransaction): boolean {
		return tx.blocking_reason !== null && tx.blocking_reason !== undefined && String(tx.blocking_reason).length > 0;
	}

	onMount(load);
</script>

<div class="ui-page-doc animate-fade-in">
	<PageHeader title={t('sidebar.transactions')}>
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

	<TransactionPanel
		overview={overview}
		transactionSnapshot={transactionSnapshot}
		transactions={activeTransactions}
		onKill={killTransaction}
	/>

	<section class="ui-card shadow-sm p-6">
		<h2 class="ui-section-title">
			{t('monitoring.activeTransactions')} ({activeTransactions.length})
		</h2>
		{#if activeTransactions.length === 0}
			<p class="text-sm text-muted">{t('monitoring.noData')}</p>
		{:else}
			<div class="overflow-x-auto">
				<table class="ui-table ui-table-dense min-w-full font-mono">
					<thead>
						<tr>
							<th>id</th>
							<th>{t('common.status')}</th>
							<th>owner</th>
							<th>{t('monitoring.duration')}</th>
							<th>staged</th>
							<th>undo</th>
							<th>{t('monitoring.blocking')}</th>
							<th>{t('common.actions')}</th>
						</tr>
					</thead>
					<tbody>
						{#each activeTransactions as txn (txn.transaction_id)}
							<tr>
								<td>{txn.transaction_id}</td>
								<td>{txn.state}</td>
								<td>{txn.owner}</td>
								<td>{formatLatencyMs(txn.elapsed_ms)}</td>
								<td>{txn.staged_bytes}</td>
								<td>{txn.undo_bytes}</td>
								<td>{hasBlockingReason(txn) ? String(txn.blocking_reason) : '—'}</td>
								<td>
									<button
										class="ui-link-danger"
										onclick={() => killTransaction(txn.transaction_id)}
										disabled={loading}
									>
										{t('monitoring.kill')}
									</button>
									{#if hasBlockingReason(txn)}
										<button
											class="ui-link ml-2"
											onclick={() => retryTransactionOutbox(txn.transaction_id)}
											disabled={loading}
										>
											{t('monitoring.retryTxnOutbox')}
										</button>
									{/if}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</section>
</div>
