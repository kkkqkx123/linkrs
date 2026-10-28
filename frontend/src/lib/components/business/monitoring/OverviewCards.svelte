<script lang="ts">
	import { t } from '$i18n';
	import TrendPanel from '$components/business/monitoring/TrendPanel.svelte';
	import {
		formatCount,
		formatLatencyMs,
		formatLatencyUs,
		formatQps,
		formatUptimeSecs,
	} from '$utils/metricsFormat';
	import type {
		DatabaseOverviewResponse,
		OverviewResponse,
		QueryStatsResponse,
		SystemResourceResponse,
	} from '$services/statistics';

	let {
		overview,
		system,
		database,
		queries,
	}: {
		overview: OverviewResponse | null;
		system: SystemResourceResponse | null;
		database: DatabaseOverviewResponse | null;
		queries: QueryStatsResponse | null;
	} = $props();

	const trendPoints = $derived(overview?.timeseries ?? []);
</script>

<section class="ui-card shadow-sm p-6">
	<h2 class="ui-section-title">
		{t('monitoring.overview')}
	</h2>
	<div class="grid grid-cols-2 md:grid-cols-4 gap-3 text-sm">
		<div>
			<div class="text-muted text-xs">
				{t('common.status')}
			</div>
			<div class="font-mono text-ink">
				{system ? t('common.statusHealthy') : t('monitoring.noData')}
			</div>
		</div>
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.connections')}
			</div>
			<div class="font-mono text-ink">
				{formatCount(system?.connections?.active)} / {formatCount(
					system?.connections?.max,
				)}
			</div>
		</div>
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.uptime')}
			</div>
			<div class="font-mono text-ink">
				{formatUptimeSecs(system?.uptime_secs)}
			</div>
		</div>
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.qps')}
			</div>
			<div class="font-mono text-ink">
				{formatQps(database?.performance?.queries_per_second)}
			</div>
		</div>
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.avgLatency')}
			</div>
			<div class="font-mono text-ink">
				{formatLatencyMs(database?.performance?.avg_latency_ms)}
			</div>
		</div>
		<div>
			<div class="text-muted text-xs">
				p50 / p95 / p99
			</div>
			<div class="font-mono text-ink">
				{formatLatencyUs(overview?.query_latency_us?.p50)} /
				{formatLatencyUs(overview?.query_latency_us?.p95)} /
				{formatLatencyUs(overview?.query_latency_us?.p99)}
			</div>
		</div>
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.errors')}
			</div>
			<div class="font-mono text-ink">
				{formatCount(
					overview?.errors?.total ?? database?.performance?.error_total,
				)}
			</div>
		</div>
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.totalQueries')}
			</div>
			<div class="font-mono text-ink">
				{formatCount(
					database?.performance?.total_queries ?? queries?.total_queries,
				)}
			</div>
		</div>
	</div>
	<TrendPanel points={trendPoints} />
</section>
