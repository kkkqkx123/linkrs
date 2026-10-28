<script lang="ts">
	import { t } from '$i18n';
	import { formatCount } from '$utils/metricsFormat';
	import type { OverviewResponse } from '$services/statistics';

	let {
		overview,
		transactionSnapshot,
	}: {
		overview: OverviewResponse | null;
		transactionSnapshot: Record<string, unknown> | null;
	} = $props();

	// Transaction metrics endpoint is untyped; read fields defensively.
	function num(record: unknown, key: string): unknown {
		if (record && typeof record === 'object') {
			return (record as Record<string, unknown>)[key];
		}
		return undefined;
	}
</script>

<section class="ui-card shadow-sm p-6">
	<h2 class="ui-section-title">
		{t('monitoring.transactions')}
	</h2>
	<div class="space-y-1 text-sm font-mono">
		<div class="flex justify-between">
			<span class="text-muted">{t('monitoring.begun')}</span>
			<span
				>{formatCount(
					overview?.transaction?.begun ?? num(transactionSnapshot, 'begun'),
				)}</span
			>
		</div>
		<div class="flex justify-between">
			<span class="text-muted">{t('monitoring.committed')}</span>
			<span
				>{formatCount(
					overview?.transaction?.committed ??
						num(transactionSnapshot, 'committed'),
				)}</span
			>
		</div>
		<div class="flex justify-between">
			<span class="text-muted">{t('monitoring.rolledBack')}</span>
			<span
				>{formatCount(
					overview?.transaction?.rolled_back ??
						num(transactionSnapshot, 'rolled_back'),
				)}</span
			>
		</div>
		<div class="flex justify-between">
			<span class="text-muted">{t('monitoring.active')}</span>
			<span
				>{formatCount(
					overview?.transaction?.active ?? num(transactionSnapshot, 'active'),
				)}</span
			>
		</div>
	</div>
</section>
