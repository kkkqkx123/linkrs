<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from '$i18n';
	import PageHeader from '$components/layout/PageHeader.svelte';
	import ConfirmDialog from '$components/common/ConfirmDialog.svelte';
	import TransactionPanel from '$components/business/monitoring/TransactionPanel.svelte';
	import { statisticsService, type OverviewResponse, type TransactionMetrics } from '$services/statistics';
	import { operationsService, type ActiveTransaction } from '$services/operations';
	import { formatLatencyMs } from '$utils/metricsFormat';
	import { notificationStore } from '$stores/notification';
	import Button from '$components/common/Button.svelte';

	let overview = $state<OverviewResponse | null>(null);
	let transactionSnapshot = $state<TransactionMetrics | null>(null);
	let activeTransactions = $state<ActiveTransaction[]>([]);
	let loading = $state(false);

	function reportError(err: unknown) {
		notificationStore.error(
			'notification.requestFailed',
			undefined,
			err instanceof Error ? err.message : undefined,
		);
	}

	// Pending confirmation; the dialog owns the decision, the action runs only
	// after explicit confirmation.
	let pendingConfirm = $state<{
		title: string;
		message: string;
		run: () => Promise<void>;
	} | null>(null);

	async function confirmPending() {
		const pending = pendingConfirm;
		pendingConfirm = null;
		if (pending) await pending.run();
	}

	async function load() {
		loading = true;
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
			reportError(err);
		} finally {
			loading = false;
		}
	}

	async function killTransaction(id: number) {
		pendingConfirm = {
			title: t('monitoring.kill'),
			message: t('monitoring.confirmKill', { id: String(id) }),
			run: async () => {
				loading = true;
				try {
					await operationsService.killTransaction(id);
					await load();
				} catch (err) {
					reportError(err);
				} finally {
					loading = false;
				}
			},
		};
	}

	async function retryTransactionOutbox(id: number) {
		pendingConfirm = {
			title: t('monitoring.retryTxnOutbox'),
			message: t('monitoring.confirmRetryTxn', { id: String(id) }),
			run: async () => {
				loading = true;
				try {
					await operationsService.retryTransactionOutbox(id);
					await load();
				} catch (err) {
					reportError(err);
				} finally {
					loading = false;
				}
			},
		};
	}

	function hasBlockingReason(tx: ActiveTransaction): boolean {
		return tx.blocking_reason !== null && tx.blocking_reason !== undefined && String(tx.blocking_reason).length > 0;
	}

	onMount(load);
</script>

<div class="ui-page-doc animate-fade-in">
	<PageHeader title={t('sidebar.transactions')}>
		<Button variant="primary" onclick={load} disabled={loading}>
			{loading ? t('monitoring.loading') : t('common.refresh')}
		</Button>
	</PageHeader>

	<TransactionPanel
		overview={overview}
		transactionSnapshot={transactionSnapshot}
		transactions={activeTransactions}
		onKill={killTransaction}
	/>

	<section class="ui-card shadow-sm p-6">
		<h3 class="ui-section-title">
			{t('monitoring.activeTransactions')} ({activeTransactions.length})
		</h3>
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

<ConfirmDialog
	open={pendingConfirm !== null}
	title={pendingConfirm?.title ?? ''}
	message={pendingConfirm?.message ?? ''}
	danger
	busy={loading}
	onConfirm={confirmPending}
	onClose={() => (pendingConfirm = null)}
/>
