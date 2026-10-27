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
		<p class="text-xs text-muted">
			{t('graphPreview.scalarCellsIgnored', { count: graph.stats.skipped })}
		</p>
	{/if}
	<div class="flex gap-2">
		<button
			class="ui-btn ui-btn-primary"
			onclick={onOpenInGraph}
		>
			{t('graphPreview.openInGraph')}
		</button>
		<button
			class="ui-btn ui-btn-ghost"
			onclick={() => onTogglePreview(!previewActive)}
		>
			{previewActive
				? t('graphPreview.hidePreview')
				: t('graphPreview.preview')}
		</button>
	</div>
</div>
