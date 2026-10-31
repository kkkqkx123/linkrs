<script lang="ts">
	import { t } from '$i18n';
	import EmptyState from '$components/common/EmptyState.svelte';
	import LazyCytoscapeCanvas from '$components/common/LazyCytoscapeCanvas.svelte';
	import GraphPreviewControls from '$components/business/GraphPreviewControls.svelte';
	import type { ParsedGraph } from '$utils/cytoscapeConfig';
	import type { QueryResult } from '$types/query';

	// Shared graph preview: toggle + inline cytoscape canvas + empty state.
	// Owns the preview-active flag so callers do not repeat that state.
	// `result` is the data handed to `onOpenInGraph` for full workbench mode.
	let {
		graph,
		result,
		isDark = false,
		onOpenInGraph,
	}: {
		graph: ParsedGraph | null;
		result: QueryResult;
		isDark?: boolean;
		onOpenInGraph: (result: QueryResult) => void;
	} = $props();

	let previewActive = $state(false);
	const handlePreviewToggle = (active: boolean) => (previewActive = active);
</script>

{#if graph && (graph.nodes.length > 0 || graph.edges.length > 0)}
	<div class="flex flex-col gap-2 text-sm text-muted">
		<GraphPreviewControls
			{graph}
			{previewActive}
			onOpenInGraph={() => onOpenInGraph(result)}
			onTogglePreview={handlePreviewToggle}
		/>
		{#if previewActive}
			<div class="h-80 rounded border border-line relative overflow-hidden">
				<LazyCytoscapeCanvas data={graph} {isDark} />
			</div>
		{/if}
	</div>
{:else}
	<EmptyState message="{t('console.viewGraph')} - {t('graph.noData')}" />
{/if}
