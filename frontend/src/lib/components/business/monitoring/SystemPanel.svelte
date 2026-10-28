<script lang="ts">
	import { t } from '$i18n';
	import {
		formatBytes,
		formatCount,
		formatPercent,
		formatPermille,
	} from '$utils/metricsFormat';
	import type {
		DatabaseOverviewResponse,
		SystemResourceResponse,
	} from '$services/statistics';

	let {
		system,
		database,
		cpuRatio,
		memRatio,
		cpuExceeded,
		memExceeded,
	}: {
		system: SystemResourceResponse | null;
		database: DatabaseOverviewResponse | null;
		cpuRatio: number | null;
		memRatio: number | null;
		cpuExceeded: boolean;
		memExceeded: boolean;
	} = $props();
</script>

<!-- Resources -->
<section class="ui-card shadow-sm p-6">
	<h2 class="ui-section-title">
		{t('monitoring.resources')}
	</h2>
	<div class="grid grid-cols-2 md:grid-cols-4 gap-3 text-sm">
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.cpu')}
			</div>
			<div
				class="font-mono {cpuExceeded
					? 'text-error font-semibold'
					: 'text-ink'}"
			>
				{cpuRatio !== null ? formatPercent(cpuRatio) : '—'}
			</div>
		</div>
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.memory')}
			</div>
			<div
				class="font-mono {memExceeded
					? 'text-error font-semibold'
					: 'text-ink'}"
			>
				{formatBytes(system?.memory_usage?.used_bytes)} / {formatBytes(
					system?.memory_usage?.total_bytes,
				)}
				{#if memRatio !== null}({formatPercent(memRatio)}){/if}
			</div>
			{#if memRatio !== null}
				<div
					class="h-1.5 mt-1 bg-secondary rounded overflow-hidden"
				>
					<div
						class="h-full bg-info"
						style="width: {Math.min(100, memRatio * 100)}%"
					></div>
				</div>
			{/if}
		</div>
		<div>
			<div class="text-muted text-xs">
				{t('common.processMemory')}
			</div>
			<div class="font-mono text-ink">
				{formatBytes(system?.process_memory_bytes)}
			</div>
		</div>
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.dataDir')}
			</div>
			<div class="font-mono text-ink">
				{formatBytes(system?.data_dir_size_bytes)}
			</div>
		</div>
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.walDir')}
			</div>
			<div class="font-mono text-ink">
				{formatBytes(system?.wal_dir_size_bytes)}
			</div>
		</div>
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.maxFds')}
			</div>
			<div class="font-mono text-ink">
				{formatCount(system?.max_file_descriptors)}
			</div>
		</div>
	</div>
</section>

<!-- Storage -->
<section class="ui-card shadow-sm p-6">
	<h2 class="ui-section-title">
		{t('monitoring.storage')}
	</h2>
	<div class="grid grid-cols-2 md:grid-cols-4 gap-3 text-sm">
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.spaces')}
			</div>
			<div class="font-mono">{formatCount(database?.spaces?.count)}</div>
		</div>
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.vertices')}
			</div>
			<div class="font-mono">
				{formatCount(database?.spaces?.total_vertices)}
			</div>
		</div>
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.edges')}
			</div>
			<div class="font-mono">
				{formatCount(database?.spaces?.total_edges)}
			</div>
		</div>
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.totalSize')}
			</div>
			<div class="font-mono">
				{formatBytes(database?.storage?.total_size_bytes)}
			</div>
		</div>
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.indexSize')}
			</div>
			<div class="font-mono">
				{formatBytes(database?.storage?.index_size_bytes)}
			</div>
		</div>
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.dataSize')}
			</div>
			<div class="font-mono">
				{formatBytes(database?.storage?.data_size_bytes)}
			</div>
		</div>
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.fragmentation')}
			</div>
			<div class="font-mono">
				{formatPermille(database?.storage?.fragmentation_permille)} ({formatBytes(
					database?.storage?.wasted_bytes,
				)})
			</div>
		</div>
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.tombstones')}
			</div>
			<div class="font-mono">
				{formatCount(database?.storage?.tombstone_count)} ({formatBytes(
					database?.storage?.tombstone_memory_bytes,
				)})
			</div>
		</div>
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.dirtyPages')}
			</div>
			<div class="font-mono">
				{formatCount(database?.storage?.dirty_pages)} / {formatCount(
					database?.storage?.dirty_pages_total,
				)}
			</div>
		</div>
		<div>
			<div class="text-muted text-xs">
				{t('monitoring.checkpointSuccess')}
			</div>
			<div class="font-mono">
				{formatCount(database?.storage?.checkpoint?.success_count)} / {formatCount(
					database?.storage?.checkpoint?.failure_count,
				)}
			</div>
		</div>
	</div>
</section>
