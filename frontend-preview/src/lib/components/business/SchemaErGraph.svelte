<script lang="ts">
	import { t } from '$i18n';
	import { SvelteSet } from 'svelte/reactivity';
	import { buildSchemaGraph } from '$utils/schemaGraph';
	import CytoscapeCanvas from '$components/common/CytoscapeCanvas.svelte';
	import type { Tag, EdgeType } from '$types/schema';

	/**
	 * Interactive ER view of the schema. Clicking a Tag or EdgeType opens a
	 * detail panel with its fields and an entry point for the alter modal; the
	 * search box dims non-matching entities. Element data flows in through props,
	 * so schema changes from any tab update the graph incrementally.
	 */
	let {
		tags = [],
		edgeTypes = [],
		isDark = false,
		onAlterTag,
		onAlterEdge,
	}: {
		tags?: Tag[];
		edgeTypes?: EdgeType[];
		isDark?: boolean;
		onAlterTag?: (name: string) => void;
		onAlterEdge?: (name: string) => void;
	} = $props();

	const schemaGraph = $derived(buildSchemaGraph(tags, edgeTypes));
	const isEmpty = $derived(schemaGraph.graph.nodes.length === 0);

	let search = $state('');
	let selected = $state<{
		id: string;
		kind: 'tag' | 'edge';
		fields: { name: string; type: string; nullable: boolean }[];
	} | null>(null);

	function handleNodeTap(data: {
		id: string;
		_tag?: string;
		props?: Record<string, unknown>;
	}) {
		const id = data.id;
		const tag = tags.find((item) => item.name === id);
		if (tag) {
			selected = {
				id,
				kind: 'tag',
				fields: (tag.properties ?? []).map((p) => ({
					name: p.name,
					type: String(p.data_type ?? ''),
					nullable: Boolean(p.nullable),
				})),
			};
			return;
		}
		const edge = edgeTypes.find((item) => item.name === id);
		if (edge) {
			selected = {
				id,
				kind: 'edge',
				fields: (edge.properties ?? []).map((p) => ({
					name: p.name,
					type: String(p.data_type ?? ''),
					nullable: Boolean(p.nullable),
				})),
			};
			return;
		}
		selected = null;
	}

	// Dim entities that do not match the search instead of removing them, so the
	// layout stays stable while typing.
	const query = $derived(search.trim().toLowerCase());
	const matchIds = $derived.by(() => {
		if (!query) return null;
		const ids = new SvelteSet<string>();
		for (const item of [...tags, ...edgeTypes]) {
			if (item.name.toLowerCase().includes(query)) ids.add(item.name);
		}
		return ids;
	});

	const canvasData = $derived.by(() => {
		const graph = schemaGraph.graph;
		if (!matchIds) return graph;
		return {
			...graph,
			nodes: graph.nodes.map((n) =>
				matchIds.has(n.id) || !matchIds.has(n.id)
					? { ...n, properties: { ...(n.properties ?? {}), _dim: matchIds.has(n.id) ? '0' : '1' } }
					: n,
			),
		};
	});
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
		<input
			type="text"
			class="ml-auto px-2 py-1 border border-gray-300 dark:border-gray-600 rounded text-xs bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200 w-40"
			placeholder={t('common.search')}
			value={search}
			oninput={(e) => (search = (e.target as HTMLInputElement).value)}
		/>
		<span>
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
				data={canvasData}
				styleConfig={schemaGraph.styles}
				{isDark}
				layout="circle"
				onNodeTap={handleNodeTap}
				onBackgroundTap={() => (selected = null)}
			/>
		{/if}
	</div>

	{#if selected}
		<div
			class="rounded border border-gray-200 dark:border-gray-700 bg-white dark:bg-[#1C2333] px-4 py-3 text-sm"
		>
			<div class="flex items-center justify-between">
				<span class="font-medium text-gray-800 dark:text-gray-100"
					>{selected.id}</span
				>
				<div class="flex items-center gap-2">
					{#if selected.kind === 'tag' && onAlterTag}
						<button
							class="px-2 py-0.5 text-xs border border-gray-300 dark:border-gray-600 rounded hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 cursor-pointer"
							onclick={() => onAlterTag!(selected!.id)}
						>
							{t('schema.alterTag')}
						</button>
					{:else if selected.kind === 'edge' && onAlterEdge}
						<button
							class="px-2 py-0.5 text-xs border border-gray-300 dark:border-gray-600 rounded hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 cursor-pointer"
							onclick={() => onAlterEdge!(selected!.id)}
						>
							{t('schema.alterEdge')}
						</button>
					{/if}
					<button
						class="text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 cursor-pointer"
						onclick={() => (selected = null)}
						aria-label={t('common.close')}>✕</button
					>
				</div>
			</div>
			<div class="text-xs text-gray-500 dark:text-gray-400 mt-1 uppercase tracking-wide">
				{selected.kind === 'tag'
					? t('schema.erLegendTag')
					: t('schema.erLegendEdge')}
			</div>
			{#if selected.fields.length > 0}
				<div class="mt-2 grid grid-cols-2 gap-x-4 gap-y-1">
					{#each selected.fields as f (f.name)}
						<div class="text-xs flex gap-1 font-mono">
							<span class="text-gray-800 dark:text-gray-200">{f.name}</span>
							<span class="text-gray-400 dark:text-gray-500">{f.type}</span>
							{#if !f.nullable}
								<span class="text-amber-600 dark:text-amber-400">required</span>
							{/if}
						</div>
					{/each}
				</div>
			{:else}
				<div class="text-xs text-gray-400 dark:text-gray-500 mt-1">
					{t('common.noProperties')}
				</div>
			{/if}
		</div>
	{/if}
</div>
