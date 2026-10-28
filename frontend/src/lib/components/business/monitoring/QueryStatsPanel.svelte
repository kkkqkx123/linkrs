<script lang="ts">
	import { t } from '$i18n';
	import { goto } from '$app/navigation';
	import { consoleStore } from '$stores/console';
	import EmptyState from '$components/common/EmptyState.svelte';
	import {
		formatCount,
		formatLatencyMs,
		formatLatencyUs,
		formatPercent,
	} from '$utils/metricsFormat';
	import type {
		DatabaseOverviewResponse,
		QueryStatsResponse,
	} from '$services/statistics';

	type PatternEntry = NonNullable<QueryStatsResponse['top_patterns']>[number];
	type ExecutorEntry = NonNullable<
		QueryStatsResponse['executor_summary']
	>[number];

	let {
		queries,
		database,
		patterns,
		executors,
	}: {
		queries: QueryStatsResponse | null;
		database: DatabaseOverviewResponse | null;
		patterns: PatternEntry[];
		executors: ExecutorEntry[];
	} = $props();

	let copiedPattern = $state<string | null>(null);

	async function copyStatement(pattern: PatternEntry) {
		try {
			await navigator.clipboard.writeText(pattern.normalized_query);
			copiedPattern = pattern.normalized_query;
			setTimeout(() => {
				if (copiedPattern === pattern.normalized_query) copiedPattern = null;
			}, 1500);
		} catch {
			/* clipboard unavailable */
		}
	}

	function runInConsole(pattern: PatternEntry) {
		consoleStore.setEditorContent(pattern.normalized_query);
		goto('/console');
	}
</script>

<section class="ui-card shadow-sm p-6">
	<h2 class="ui-section-title">
		{t('monitoring.queries')}
	</h2>
	<div class="grid grid-cols-2 md:grid-cols-4 gap-3 text-sm mb-4">
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.totalQueries')}
			</div>
			<div class="font-mono text-ink">
				{formatCount(queries?.total_queries)}
			</div>
		</div>
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.active')}
			</div>
			<div class="font-mono text-ink">
				{formatCount(database?.performance?.active_queries)}
			</div>
		</div>
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.avgLatency')}
			</div>
			<div class="font-mono text-ink">
				{formatLatencyUs(queries?.latency_percentiles_us?.avg)}
			</div>
		</div>
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.errors')}
			</div>
			<div class="font-mono text-ink">
				{formatCount(queries?.error_total)}
			</div>
		</div>
	</div>

	{#if queries}
		<div class="grid md:grid-cols-2 gap-4 mb-4 text-sm">
			<div>
				<h3 class="text-xs font-medium text-muted mb-1">
					{t('monitoring.errorsByType')}
				</h3>
				{#if Object.keys(queries.errors_by_type ?? {}).length === 0}
					<EmptyState message={t('monitoring.noData')} />
				{:else}
					<ul class="space-y-1">
						{#each Object.entries(queries.errors_by_type ?? {}) as [kind, count] (kind)}
							<li class="flex justify-between font-mono">
								<span class="text-muted">{kind}</span>
								<span class="text-ink"
									>{formatCount(count)}</span
								>
							</li>
						{/each}
					</ul>
				{/if}
			</div>
			<div>
				<h3 class="text-xs font-medium text-muted mb-1">
					{t('monitoring.errorsByPhase')}
				</h3>
				{#if Object.keys(queries.errors_by_phase ?? {}).length === 0}
					<EmptyState message={t('monitoring.noData')} />
				{:else}
					<ul class="space-y-1">
						{#each Object.entries(queries.errors_by_phase ?? {}) as [phase, count] (phase)}
							<li class="flex justify-between font-mono">
								<span class="text-muted">{phase}</span>
								<span class="text-ink"
									>{formatCount(count)}</span
								>
							</li>
						{/each}
					</ul>
				{/if}
			</div>
		</div>
	{/if}

	<h3 class="text-xs font-medium text-muted mb-1">
		{t('monitoring.topPatterns')}
	</h3>
	<div class="overflow-x-auto mb-4">
		{#if patterns.length === 0}
			<EmptyState message={t('monitoring.noData')} />
		{:else}
			<table class="ui-table ui-table-dense min-w-full">
				<thead>
					<tr>
						<th>{t('monitoring.pattern')}</th>
						<th>{t('monitoring.count')}</th>
						<th>{t('monitoring.avg')}</th>
						<th>p95</th>
						<th>p99</th>
						<th>{t('monitoring.errorRate')}</th>
						<th>{t('common.actions')}</th>
					</tr>
				</thead>
				<tbody>
					{#each patterns as p (p.normalized_query)}
						<tr class="font-mono">
							<td class="max-w-72 truncate" title={p.normalized_query}>
								{p.normalized_query}
							</td>
							<td>{formatCount(p.execution_count)}</td>
							<td>{formatLatencyMs(p.avg_duration_ms)}</td>
							<td>{formatLatencyMs(p.p95_duration_ms)}</td>
							<td>{formatLatencyMs(p.p99_duration_ms)}</td>
							<td>{formatPercent(p.error_rate)}</td>
							<td>
								<div class="flex items-center gap-2 whitespace-nowrap">
									<button
										class="ui-btn ui-btn-soft ui-btn-xs"
										onclick={() => copyStatement(p)}
									>
										{copiedPattern === p.normalized_query
											? t('monitoring.copied')
											: t('monitoring.copyStatement')}
									</button>
									<button
										class="ui-btn ui-btn-soft ui-btn-xs"
										onclick={() => runInConsole(p)}
									>
										{t('monitoring.runInConsole')}
									</button>
								</div>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
	</div>

	<h3 class="text-xs font-medium text-muted mb-1">
		{t('monitoring.executors')}
	</h3>
	<div class="overflow-x-auto">
		{#if executors.length === 0}
			<EmptyState message={t('monitoring.noData')} />
		{:else}
			<table class="ui-table ui-table-dense min-w-full">
				<thead>
					<tr>
						<th>{t('monitoring.executor')}</th>
						<th>{t('monitoring.count')}</th>
						<th>{t('monitoring.totalTime')}</th>
						<th>{t('monitoring.totalRows')}</th>
					</tr>
				</thead>
				<tbody>
					{#each executors as e (e.executor_type)}
						<tr class="font-mono">
							<td>{e.executor_type}</td>
							<td>{formatCount(e.count)}</td>
							<td>{formatLatencyMs(e.total_time_ms)}</td>
							<td>{formatCount(e.total_rows)}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		{/if}
	</div>
</section>
