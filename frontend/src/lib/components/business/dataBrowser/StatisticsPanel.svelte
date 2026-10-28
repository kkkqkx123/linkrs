<script lang="ts">
	import { t } from '$i18n';
	import type { Statistics } from '$types/dataBrowser';

	let { statistics }: { statistics: Statistics | null } = $props();
</script>

<div class="w-64 border-l border-line p-5 bg-secondary overflow-y-auto">
	<h2 class="text-base font-semibold text-ink mb-3">
		{t('dataBrowser.statistics')}
	</h2>
	{#if statistics}
		<div class="space-y-2 text-sm">
			<div class="flex justify-between">
				<span class="text-muted"
					>{t('dataBrowser.vertices')}:</span
				><span class="font-medium text-ink"
					>{statistics.totalVertices ?? '-'}</span
				>
			</div>
			<div class="flex justify-between">
				<span class="text-muted"
					>{t('dataBrowser.edges')}:</span
				><span class="font-medium text-ink"
					>{statistics.totalEdges ?? '-'}</span
				>
			</div>
			<div class="flex justify-between">
				<span class="text-muted"
					>{t('dataBrowser.tags')}:</span
				><span class="font-medium text-ink"
					>{statistics.tagCount ?? '-'}</span
				>
			</div>
			<div class="flex justify-between">
				<span class="text-muted"
					>{t('dataBrowser.edgeTypes')}:</span
				><span class="font-medium text-ink"
					>{statistics.edgeTypeCount ?? '-'}</span
				>
			</div>
		</div>
	{:else}
		<p class="text-muted text-xs">
			{t('common.refresh')}
			{t('common.loading')}
		</p>
	{/if}
</div>
