<script lang="ts">
	import { t } from '$i18n';
	import { buildSchemaGraph } from '$utils/schemaGraph';
	import CytoscapeCanvas from '$components/common/CytoscapeCanvas.svelte';
	import type { Tag, EdgeType } from '$types/schema';

	let {
		tags = [],
		edgeTypes = [],
		isDark = false,
	}: {
		tags?: Tag[];
		edgeTypes?: EdgeType[];
		isDark?: boolean;
	} = $props();

	const schemaGraph = $derived(buildSchemaGraph(tags, edgeTypes));
	const isEmpty = $derived(schemaGraph.graph.nodes.length === 0);
	let selected = $state<{
		id: string;
		kind: string;
		properties: string;
	} | null>(null);

	function handleNodeTap(data: {
		id: string;
		_tag?: string;
		props?: Record<string, unknown>;
	}) {
		selected = {
			id: data.id,
			kind: String(data.props?.kind ?? ''),
			properties: String(data.props?.properties ?? ''),
		};
	}
</script>

<div class="flex flex-col gap-3 h-[calc(100vh-280px)] min-h-[400px]">
	<div class="flex items-center gap-4 text-xs text-gray-500 dark:text-gray-400">
		<span class="flex items-center gap-1.5">
			<span class="inline-block w-3 h-3 rounded-full bg-blue-500"></span>
			{t('schema.erLegendTag')}
		</span>
		<span class="flex items-center gap-1.5">
			<span class="inline-block w-3 h-3 rounded-full bg-purple-500"></span>
			{t('schema.erLegendEdge')}
		</span>
		<span class="ml-auto">
			{schemaGraph.graph.nodes.length}
			{t('common.entity')}
		</span>
	</div>

	<div
		class="flex-1 relative rounded border border-gray-200 dark:border-gray-700 overflow-hidden bg-gray-50 dark:bg-gray-900/30"
	>
		{#if isEmpty}
			<div
				class="absolute inset-0 flex items-center justify-center text-sm text-gray-400 dark:text-gray-500"
			>
				{t('schema.erNoData')}
			</div>
		{:else}
			<CytoscapeCanvas
				data={schemaGraph.graph}
				styleConfig={schemaGraph.styles}
				{isDark}
				layout="circle"
				onNodeTap={handleNodeTap}
			/>
		{/if}
	</div>

	{#if selected}
		<div
			class="rounded border border-gray-200 dark:border-gray-700 bg-white dark:bg-[#1C2333] px-4 py-2 text-sm"
		>
			<div class="flex items-center justify-between">
				<span class="font-medium text-gray-800 dark:text-gray-100"
					>{selected.id}</span
				>
				<button
					class="text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 cursor-pointer"
					onclick={() => (selected = null)}
					aria-label={t('common.close')}>✕</button
				>
			</div>
			<div class="text-xs text-gray-500 dark:text-gray-400 mt-1">
				<span class="uppercase tracking-wide">{selected.kind}</span>
				{#if selected.properties}
					<span class="ml-2 font-mono break-all">{selected.properties}</span>
				{/if}
			</div>
		</div>
	{/if}
</div>
