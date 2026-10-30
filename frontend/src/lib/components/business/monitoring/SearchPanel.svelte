<script lang="ts">
	import { t } from '$i18n';
	import {
		formatCount,
		formatLatencyMs,
		formatPercent,
	} from '$utils/metricsFormat';
	import type { SearchStatsResponse } from '$services/statistics';

	type ByIndexEntry = NonNullable<
		SearchStatsResponse['by_index']
	>[number];

	let {
		search,
		byIndex,
	}: {
		search: SearchStatsResponse | null;
		byIndex: ByIndexEntry[];
	} = $props();

	function avgMs(
		totalMs: number | undefined,
		count: number | undefined,
	): number | undefined {
		if (totalMs === undefined || !count) return undefined;
		return totalMs / count;
	}
</script>

<section class="ui-card shadow-sm p-6">
	<h2 class="ui-section-title">
		{t('common.search')}
	</h2>
	<p class="text-xs text-muted mb-2">{t('monitoring.retrievalSearchNote')}</p>
	<ul class="ui-kv-list text-sm font-mono">
		<li>
			<span class="text-muted">{t('monitoring.totalQueries')}</span>
			<span>{formatCount(search?.search?.total_queries)}</span>
		</li>
		<li>
			<span class="text-muted">{t('monitoring.avgLatency')}</span>
			<span>{formatLatencyMs(search?.search?.avg_latency_ms)}</span>
		</li>
		<li>
			<span class="text-muted">{t('monitoring.cacheHitRate')}</span>
			<span>{formatPercent(search?.cache?.hit_rate)}</span>
		</li>
		{#if search?.vector}
			<li>
				<span class="text-muted">{t('monitoring.vectorQueries')}</span>
				<span>
					{formatCount(search.vector.total_queries)}
					<span class="text-muted">
						({t('monitoring.errorCount', { count: formatCount(search.vector.total_errors) })})
					</span>
				</span>
			</li>
			<li>
				<span class="text-muted">{t('monitoring.vectorAvgLatency')}</span>
				<span>{formatLatencyMs(avgMs(search.vector.total_latency_ms, search.vector.total_queries))}</span>
			</li>
		{/if}
		{#if search?.embedding}
			<li>
				<span class="text-muted">{t('monitoring.embeddingCalls')}</span>
				<span>
					{formatCount(search.embedding.total_operations)}
					<span class="text-muted">
						({t('monitoring.errorCount', { count: formatCount(search.embedding.total_errors) })})
					</span>
				</span>
			</li>
			<li>
				<span class="text-muted">{t('monitoring.embeddingAvgLatency')}</span>
				<span>{formatLatencyMs(avgMs(search.embedding.total_latency_ms, search.embedding.total_operations))}</span>
			</li>
		{/if}
		{#if search?.rerank}
			<li>
				<span class="text-muted">{t('monitoring.rerankCalls')}</span>
				<span>
					{formatCount(search.rerank.total_operations)}
					<span class="text-muted">
						({t('monitoring.errorCount', { count: formatCount(search.rerank.total_errors) })})
					</span>
				</span>
			</li>
			<li>
				<span class="text-muted">{t('monitoring.rerankAvgLatency')}</span>
				<span>{formatLatencyMs(avgMs(search.rerank.total_latency_ms, search.rerank.total_operations))}</span>
			</li>
		{/if}
	</ul>
	{#if byIndex.length > 0}
		<p
			class="text-xs font-medium text-muted mt-3 mb-1"
		>
			{t('monitoring.byIndex')}
		</p>
		<div class="overflow-x-auto">
			<table class="ui-table ui-table-dense min-w-full font-mono">
				<tbody>
					{#each byIndex.slice(0, 8) as entry (entry.index)}
						<tr>
							<td class="max-w-40 truncate">{entry.index}</td>
							<td>{formatCount(entry.search_queries)}</td>
							<td>{formatLatencyMs(entry.avg_search_latency_ms)}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</section>
