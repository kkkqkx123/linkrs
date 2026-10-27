<script lang="ts">
	// Lazy facade for the graph canvas: cytoscape and the canvas component are
	// only fetched when a preview is actually rendered, so routes whose result
	// views merely *may* show a graph keep the ~800 KiB library out of their
	// initial chunk.
	import type { Component } from 'svelte';
	import type { GraphData } from '$types/graph';

	let { data, isDark = false }: { data: GraphData; isDark?: boolean } =
		$props();

	let Canvas: Component<{ data: GraphData; isDark?: boolean }> | null =
		$state(null);

	$effect(() => {
		import('$components/common/CytoscapeCanvas.svelte').then((module) => {
			Canvas = module.default;
		});
	});
</script>

{#if Canvas}
	<Canvas {data} {isDark} />
{:else}
	<div
		class="h-full w-full flex items-center justify-center text-sm text-muted"
	>
		<span class="animate-pulse">Loading graph…</span>
	</div>
{/if}
