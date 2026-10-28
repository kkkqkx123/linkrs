<script lang="ts">
	import { t } from '$i18n';
	import TrendChart from '$components/common/TrendChart.svelte';
	import {
		formatCount,
		formatLatencyMs,
	} from '$utils/metricsFormat';
	import type { OverviewResponse } from '$services/statistics';

	let { points }: { points: OverviewResponse['timeseries'] } = $props();

	// Charts are split by unit so series on one chart share a scale:
	// throughput (queries/s + errors/s) on one, latency (ms) on the other.
	const throughputSeries = $derived.by(() => {
		if (points.length === 0) return [];
		return [
			{
				key: 'queries',
				label: t('monitoring.qps'),
				color: '#3b82f6',
				values: points.map((p) => Number(p.queries) || 0),
			},
			{
				key: 'errors',
				label: t('monitoring.errors'),
				color: '#ef4444',
				values: points.map((p) => Number(p.errors) || 0),
			},
		];
	});

	const latencySeries = $derived.by(() => {
		if (points.length === 0) return [];
		return [
			{
				key: 'latency',
				label: t('monitoring.avgLatency'),
				color: '#14b8a6',
				values: points.map((p) => Number(p.avg_latency_ms) || 0),
			},
		];
	});

	const xLabels = $derived(
		points.map((p) => {
			const second = Number(p.second) || 0;
			if (!second) return '';
			return new Date(second * 1000).toLocaleTimeString([], {
				hour: '2-digit',
				minute: '2-digit',
			});
		}),
	);
</script>

{#if points.length > 0}
	<div class="grid md:grid-cols-2 gap-4">
		<div>
			<div class="text-xs text-muted mb-1">
				{t('monitoring.trend')}: {formatCount(
					points.reduce((a, p) => a + (Number(p.queries) || 0), 0),
				)}
				{t('monitoring.queriesAvg')}
				{formatLatencyMs(
					points.reduce(
						(a, p) => a + (Number(p.avg_latency_ms) || 0),
						0,
					) / Math.max(1, points.length),
				)}
			</div>
			<TrendChart series={throughputSeries} xLabels={xLabels} height={120} />
		</div>
		<div>
			<div class="text-xs text-muted mb-1">
				{t('monitoring.avgLatency')} (ms)
			</div>
			<TrendChart series={latencySeries} xLabels={xLabels} height={120} />
		</div>
	</div>
{/if}
