<script lang="ts">
	import { t } from '$i18n';
	import type { ParsedGraph } from '$utils/cytoscapeConfig';

	let {
		graph,
		previewActive = false,
		onOpenInGraph,
		onTogglePreview,
	}: {
		graph: ParsedGraph;
		previewActive?: boolean;
		onOpenInGraph: () => void;
		onTogglePreview: (active: boolean) => void;
	} = $props();
</script>

<div class="flex flex-col items-center gap-2">
	<p>
		🔗 {graph.stats.truncated
			? t('graphPreview.summaryTruncated', {
					nodes: graph.nodes.length,
					edges: graph.edges.length,
				})
			: t('graphPreview.summary', {
					nodes: graph.nodes.length,
					edges: graph.edges.length,
				})}
	</p>
	{#if graph.stats.skipped > 0}
		<p class="text-xs text-gray-400">
			{t('graphPreview.scalarCellsIgnored', { count: graph.stats.skipped })}
		</p>
	{/if}
	<div class="flex gap-2">
		<button
			class="px-4 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer"
			onclick={onOpenInGraph}
		>
			{t('graphPreview.openInGraph')}
		</button>
		<button
			class="px-4 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer"
			onclick={() => onTogglePreview(!previewActive)}
		>
			{previewActive
				? t('graphPreview.hidePreview')
				: t('graphPreview.preview')}
		</button>
	</div>
</div>
