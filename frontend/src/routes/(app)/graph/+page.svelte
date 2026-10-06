<script lang="ts">
	import { SvelteSet } from 'svelte/reactivity';
	import { get } from 'svelte/store';

	import { t } from '$i18n';
	import { graphStore, type EdgeDetail, type GraphState, type NodeDetail } from '$stores/graph';
	import { schemaStore } from '$stores/schema';
	import { notificationStore } from '$stores/notification';
	import { graphService } from '$services/graph';
	import { theme } from '$stores/theme';
	import { getLayoutOptions } from '$utils/graphLayout';
	import { makeEdgeId } from '$utils/cytoscapeConfig';
	import CytoscapeCanvas from '$components/common/CytoscapeCanvas.svelte';
	import type { GraphStyleConfig, LayoutType } from '$types/graph';
	import type cytoscape from 'cytoscape';

	let nodeLabelFields = $state<Record<string, string[]>>({});
	let edgeLabelFields = $state<Record<string, string[]>>({});
	let stylePanelOpen = $state(false);
	let filterPanelOpen = $state(false);
	let cyInstance = $state<cytoscape.Core | null>(null);
	let relayoutToken = $state(0);
	let isExpanding = $state(false);
	let initError = $state<string | null>(null);
	let layoutDuration = $state(0);
	let syncDuration = $state(0);
	const expandedNodes = new SvelteSet<string>();
	const failedNodes = new SvelteSet<string>();

	let storeState = $state<GraphState>(get(graphStore));
	$effect(() => graphStore.subscribe((s) => { storeState = s; }));

	let layout = $derived(storeState.layout);
	let graphData = $derived(storeState.graphData);
	let detailPanelVisible = $derived(storeState.detailPanelVisible);
	let detailData = $derived(storeState.detailData);
	let detailType = $derived(storeState.detailType);
	let nodeStyles = $derived(storeState.nodeStyles);
	let edgeStyles = $derived(storeState.edgeStyles);
	let storeZoom = $derived(storeState.zoom);
	let isDark = $derived(get(theme) === 'dark');
	let searchQuery = $derived(storeState.searchQuery);
	let filterTags = $derived(storeState.filterTags);
	let filterEdgeTypes = $derived(storeState.filterEdgeTypes);
	let simplifiedMode = $derived(storeState.simplifiedMode);
	let layoutParams = $derived(storeState.layoutParams);

	const layoutOptions = getLayoutOptions();

	function buildStyleConfig(): GraphStyleConfig {
		return {
			nodes: Object.fromEntries(
				Object.entries(nodeStyles).map(([k, v]) => [
					k,
					{ color: v.color, size: v.size, labelProperty: v.labelProperty },
				]),
			),
			edges: Object.fromEntries(
				Object.entries(edgeStyles).map(([k, v]) => [
					k,
					{ color: v.color, width: v.width, labelProperty: v.labelProperty },
				]),
			),
		};
	}

	const styleConfig = $derived(buildStyleConfig());

	$effect(() => {
		if (graphData) {
			const nFields: Record<string, Set<string>> = {};
			const eFields: Record<string, Set<string>> = {};
			for (const n of graphData.nodes) {
				nFields[n.tag] ??= new Set();
				for (const k of Object.keys(n.properties)) nFields[n.tag].add(k);
			}
			for (const e of graphData.edges) {
				eFields[e.type] ??= new Set();
				for (const k of Object.keys(e.properties)) eFields[e.type].add(k);
			}
			nodeLabelFields = Object.fromEntries(
				Object.entries(nFields).map(([k, v]) => [k, [...v]]),
			);
			edgeLabelFields = Object.fromEntries(
				Object.entries(eFields).map(([k, v]) => [k, [...v]]),
			);
		} else {
			nodeLabelFields = {};
			edgeLabelFields = {};
		}
	});

	$effect(() => {
		const cy = cyInstance;
		if (!cy || !graphData) return;
		const query = searchQuery.toLowerCase();
		const hasFilter = filterTags.size > 0 || filterEdgeTypes.size > 0;
		const hasSearch = query.length > 0;

		cy.batch(() => {
			cy.elements().removeClass('search-match search-dimmed filter-hidden simplified-hidden');

			if (!hasSearch && !hasFilter && !simplifiedMode) return;

			if (hasSearch) {
				cy.nodes().forEach((node) => {
					const data = node.data() as Record<string, unknown>;
					const id = String(data.id ?? '').toLowerCase();
					const label = String(data.label ?? '').toLowerCase();
					const props = data.props as Record<string, unknown> | undefined;
					const propMatch = props
						? Object.values(props).some((v) =>
								String(v).toLowerCase().includes(query),
							)
						: false;
					if (id.includes(query) || label.includes(query) || propMatch) {
						node.addClass('search-match');
					} else {
						node.addClass('search-dimmed');
					}
				});
			}

			if (hasFilter) {
				cy.nodes().forEach((node) => {
					const tag = node.data('_tag') as string;
					if (filterTags.size > 0 && !filterTags.has(tag)) {
						node.addClass('filter-hidden');
					}
				});
				cy.edges().forEach((edge) => {
					const type = edge.data('_type') as string;
					if (filterEdgeTypes.size > 0 && !filterEdgeTypes.has(type)) {
						edge.addClass('filter-hidden');
					}
				});
			}

			if (simplifiedMode) {
				cy.nodes().forEach((node) => {
					const degree = node.degree(false);
					if (degree <= 1) {
						node.addClass('simplified-hidden');
					}
				});
			}
		});
	});

	function handleNodeTap(data: {
		id: string;
		_tag?: string;
		label?: string;
		props?: Record<string, unknown>;
	}) {
		graphStore.selectNode(data.id);
		const detail: NodeDetail = {
			id: data.id,
			tag: data._tag || 'unknown',
			properties: data.props ?? {},
		};
		graphStore.showDetail(detail, 'node');
		void expandNode(data.id);
	}

	function handleEdgeTap(data: {
		id: string;
		source: string;
		target: string;
		_type?: string;
		_rank?: number;
		props?: Record<string, unknown>;
	}) {
		graphStore.selectEdge(data.id);
		const detail: EdgeDetail = {
			id: data.id,
			type: data._type || 'unknown',
			source: data.source,
			target: data.target,
			rank: data._rank || 0,
			properties: data.props ?? {},
		};
		graphStore.showDetail(detail, 'edge');
	}

	function handleBackgroundTap() {
		if (cyInstance) cyInstance.elements().unselect();
		graphStore.clearSelection();
	}

	async function expandNode(id: string) {
		if (expandedNodes.has(id)) return;
		const space = get(schemaStore).currentSpace;
		if (!space) {
			notificationStore.warning('notification.selectSpaceFirst');
			return;
		}
		expandedNodes.add(id);
		failedNodes.delete(id);
		isExpanding = true;
		try {
			const neighbors = await graphService.vertices.getNeighbors(id, space);
			const nodes = neighbors.map((n) => ({
				id: n.vid,
				tag: n.tag,
				properties: n.properties,
			}));
			const edges = neighbors.map((n) => {
				const source = n.direction === 'OUT' ? id : n.vid;
				const target = n.direction === 'OUT' ? n.vid : id;
				return {
					id: makeEdgeId(source, target, n.edge_type, n.rank),
					type: n.edge_type,
					source,
					target,
					rank: n.rank,
					properties: {},
				};
			});
			graphStore.mergeGraphData({ nodes, edges });
			relayoutToken += 1;
		} catch (err) {
			expandedNodes.delete(id);
			failedNodes.add(id);
			notificationStore.error(
				'notification.loadNeighborsFailed',
				undefined,
				err instanceof Error ? err.message : undefined,
			);
		} finally {
			isExpanding = false;
		}
	}

	function retryExpand(id: string) {
		expandedNodes.delete(id);
		void expandNode(id);
	}

	function handleLayoutChange(e: Event) {
		const val = (e.target as HTMLSelectElement).value as LayoutType;
		graphStore.setLayout(val);
	}

	function handleClearGraph() {
		graphStore.clearGraphData();
		graphStore.hideDetail();
		expandedNodes.clear();
		cyInstance?.elements().remove();
	}

	function handleFitToScreen() {
		cyInstance?.fit(undefined, 30);
	}

	function handleResetZoom() {
		if (!cyInstance) return;
		cyInstance.zoom(1);
		cyInstance.center();
	}

	function handleExportPng() {
		if (!cyInstance) return;
		const png = cyInstance.png({ full: true, bg: isDark ? '#111827' : '#ffffff' });
		if (!png) return;
		const link = document.createElement('a');
		link.href = png;
		link.download = 'graph.png';
		link.click();
	}

	function handleExportJson() {
		if (!graphData) return;
		const blob = new Blob([JSON.stringify(graphData, null, 2)], {
			type: 'application/json',
		});
		const url = URL.createObjectURL(blob);
		const link = document.createElement('a');
		link.href = url;
		link.download = 'graph.json';
		link.click();
		URL.revokeObjectURL(url);
	}
</script>

<div class="flex flex-col h-full gap-4">
	<div
		class="bg-white dark:bg-[#1C2333] rounded-lg shadow-sm px-5 py-3 flex items-center justify-between"
	>
		<h2
			class="text-lg font-semibold text-gray-800 dark:text-gray-100 flex items-center gap-2"
		>
			<span>🔗</span>
			{t('graph.title')}
			{#if graphData}
				<span class="text-xs font-normal text-gray-500 dark:text-gray-400"
					>{t('graphPreview.summary', {
						nodes: graphData.nodes.length,
						edges: graphData.edges.length,
					})}</span
				>
			{/if}
			{#if isExpanding}
				<span class="text-xs font-normal text-blue-500 dark:text-blue-400"
					>Expanding…</span
				>
			{/if}
		</h2>
		<div class="flex items-center gap-3">
			<input
				type="text"
				class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200 focus:outline-none focus:border-blue-500 w-48"
				placeholder={t('graph.search')}
				value={searchQuery}
				oninput={(e) => graphStore.setSearchQuery((e.target as HTMLInputElement).value)}
				disabled={!graphData}
			/>
			<button
				class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer"
				onclick={() => (filterPanelOpen = !filterPanelOpen)}
				disabled={!graphData}
			>
				{t('dataBrowser.filter')}
			</button>
			<button
				class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer"
				onclick={handleFitToScreen}
				disabled={!graphData}
			>
				{t('graph.fit')}
			</button>
			<button
				class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer"
				onclick={handleResetZoom}
				disabled={!graphData}
			>
				{t('graph.reset')}
			</button>
			<button
				class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer"
				onclick={() => (stylePanelOpen = !stylePanelOpen)}
				disabled={!graphData}
			>
				{t('graph.style')}
			</button>
			<button
				class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer"
				onclick={handleExportPng}
				disabled={!graphData}
			>
				PNG
			</button>
			<button
				class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer"
				onclick={handleExportJson}
				disabled={!graphData}
			>
				JSON
			</button>
			<select
				class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200 focus:outline-none focus:border-blue-500"
				aria-label={t('graph.layoutLabel')}
				value={layout}
				onchange={handleLayoutChange}
			>
				{#each layoutOptions as opt (opt.value)}
					<option value={opt.value}>{opt.label()}</option>
				{/each}
			</select>
			<button
				class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer disabled:opacity-50"
				onclick={handleClearGraph}
				disabled={!graphData}
			>
				{t('common.clear')}
			</button>
		</div>
	</div>

	{#if filterPanelOpen && graphData}
		<div
			class="bg-white dark:bg-[#1C2333] rounded-lg shadow-sm px-5 py-3 grid grid-cols-2 gap-4 max-h-48 overflow-y-auto"
		>
			<div>
				<h4 class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">
					{t('graph.filterTags')}
				</h4>
				<div class="flex flex-wrap gap-2">
					{#each Object.keys(nodeStyles) as tag (tag)}
						<button
							class="px-2 py-1 text-xs rounded border cursor-pointer {filterTags.has(tag)
								? 'bg-blue-500 text-white border-blue-500'
								: 'bg-white dark:bg-[#1C2333] text-gray-700 dark:text-gray-300 border-gray-300 dark:border-gray-600'}"
							onclick={() => graphStore.toggleFilterTag(tag)}
						>
							{tag}
						</button>
					{/each}
				</div>
			</div>
			<div>
				<h4 class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">
					{t('graph.filterEdgeTypes')}
				</h4>
				<div class="flex flex-wrap gap-2">
					{#each Object.keys(edgeStyles) as type (type)}
						<button
							class="px-2 py-1 text-xs rounded border cursor-pointer {filterEdgeTypes.has(type)
								? 'bg-blue-500 text-white border-blue-500'
								: 'bg-white dark:bg-[#1C2333] text-gray-700 dark:text-gray-300 border-gray-300 dark:border-gray-600'}"
							onclick={() => graphStore.toggleFilterEdgeType(type)}
						>
							{type}
						</button>
					{/each}
				</div>
			</div>
			<div class="col-span-2 flex items-center gap-3">
				<label class="flex items-center gap-2 text-sm text-gray-700 dark:text-gray-300 cursor-pointer">
					<input
						type="checkbox"
						checked={simplifiedMode}
						onchange={(e) => graphStore.setSimplifiedMode((e.target as HTMLInputElement).checked)}
						class="cursor-pointer"
					/>
					{t('graph.simplifiedMode')}
				</label>
				<span class="text-xs text-gray-400">{t('graph.simplifiedModeHint')}</span>
				<button
					class="ml-auto px-3 py-1 text-xs text-gray-500 hover:text-gray-700 dark:hover:text-gray-300 cursor-pointer"
					onclick={() => graphStore.clearFilters()}
				>
					{t('graph.clearFilters')}
				</button>
			</div>
		</div>
	{/if}

	{#if stylePanelOpen && graphData}
		<div
			class="bg-white dark:bg-[#1C2333] rounded-lg shadow-sm px-5 py-3 grid grid-cols-2 gap-4 max-h-48 overflow-y-auto"
		>
			<div>
				<h4 class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">
					{t('graph.nodes')}
				</h4>
				{#each Object.entries(nodeStyles) as [tag, style] (tag)}
					<div class="flex items-center gap-2 mb-1 text-sm">
						<input
							type="color"
							value={style.color}
							oninput={(e) =>
								graphStore.setNodeStyle(tag, {
									color: (e.target as HTMLInputElement).value,
								})}
							class="w-8 h-6 cursor-pointer"
						/>
						<span class="text-gray-700 dark:text-gray-300 font-mono text-xs"
							>{tag}</span
						>
						<select
							class="ml-auto px-1 py-0.5 border border-gray-300 dark:border-gray-600 rounded text-xs bg-white dark:bg-[#1C2333] text-gray-700 dark:text-gray-300 max-w-28 truncate"
							value={style.labelProperty}
							onchange={(e) =>
								graphStore.setNodeStyle(tag, {
									labelProperty: (e.target as HTMLSelectElement).value,
								})}
						>
							<option value="id">id</option>
							{#each nodeLabelFields[tag] ?? [] as field (field)}
								<option value={field}>{field}</option>
							{/each}
						</select>
					</div>
				{/each}
			</div>
			<div>
				<h4 class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">
					{t('graph.edges')}
				</h4>
				{#each Object.entries(edgeStyles) as [type, style] (type)}
					<div class="flex items-center gap-2 mb-1 text-sm">
						<input
							type="color"
							value={style.color}
							oninput={(e) =>
								graphStore.setEdgeStyle(type, {
									color: (e.target as HTMLInputElement).value,
								})}
							class="w-8 h-6 cursor-pointer"
						/>
						<span class="text-gray-700 dark:text-gray-300 font-mono text-xs"
							>{type}</span
						>
						<select
							class="ml-auto px-1 py-0.5 border border-gray-300 dark:border-gray-600 rounded text-xs bg-white dark:bg-[#1C2333] text-gray-700 dark:text-gray-300 max-w-28 truncate"
							value={style.labelProperty}
							onchange={(e) =>
								graphStore.setEdgeStyle(type, {
									labelProperty: (e.target as HTMLSelectElement).value,
								})}
						>
							<option value="type">type</option>
							{#each edgeLabelFields[type] ?? [] as field (field)}
								<option value={field}>{field}</option>
							{/each}
						</select>
					</div>
				{/each}
			</div>
			<div class="col-span-2 border-t border-gray-200 dark:border-gray-700 pt-3 mt-2">
				<h4 class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">
					{t('graph.layoutLabel')}
				</h4>
				<div class="grid grid-cols-3 gap-3">
					<label class="flex flex-col gap-1 text-xs text-gray-500 dark:text-gray-400">
						{t('graph.nodeRepulsion')}
						<input
							type="number"
							class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded text-xs bg-white dark:bg-[#1C2333] text-gray-700 dark:text-gray-300"
							value={layoutParams.nodeRepulsion}
							onchange={(e) => graphStore.setLayoutParams({ nodeRepulsion: Number((e.target as HTMLInputElement).value) })}
						/>
					</label>
					<label class="flex flex-col gap-1 text-xs text-gray-500 dark:text-gray-400">
						{t('graph.gravity')}
						<input
							type="number"
							step="0.01"
							class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded text-xs bg-white dark:bg-[#1C2333] text-gray-700 dark:text-gray-300"
							value={layoutParams.gravity}
							onchange={(e) => graphStore.setLayoutParams({ gravity: Number((e.target as HTMLInputElement).value) })}
						/>
					</label>
					<label class="flex flex-col gap-1 text-xs text-gray-500 dark:text-gray-400">
						{t('graph.numIter')}
						<input
							type="number"
							class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded text-xs bg-white dark:bg-[#1C2333] text-gray-700 dark:text-gray-300"
							value={layoutParams.numIter}
							onchange={(e) => graphStore.setLayoutParams({ numIter: Number((e.target as HTMLInputElement).value) })}
						/>
					</label>
				</div>
			</div>
		</div>
	{/if}

	<div
		class="flex-1 bg-white dark:bg-[#1C2333] rounded-lg shadow-sm flex overflow-hidden"
	>
		<div class="flex-1 relative">
			{#if graphData}
			<CytoscapeCanvas
				bind:cyInstance
				data={graphData}
				{styleConfig}
				{layout}
				{isDark}
				zoom={storeZoom}
				{relayoutToken}
				{layoutParams}
				onNodeTap={handleNodeTap}
				onEdgeTap={handleEdgeTap}
				onBackgroundTap={handleBackgroundTap}
				onZoom={(z) => graphStore.setZoom(z)}
			/>
			<div class="absolute bottom-2 right-2 text-xs text-gray-400 dark:text-gray-500 bg-white/80 dark:bg-[#1C2333]/80 px-2 py-1 rounded">
				Layout: {layoutDuration.toFixed(0)}ms | Sync: {syncDuration.toFixed(0)}ms
			</div>
			{:else if initError}
				<div class="absolute inset-0 flex items-center justify-center">
					<div class="text-center text-red-500 dark:text-red-400">
						<p class="text-4xl mb-3">⚠️</p>
						<p class="font-medium">{t('graph.initError')}</p>
						<p class="text-sm mt-1">{initError}</p>
					</div>
				</div>
			{:else}
				<div class="absolute inset-0 flex items-center justify-center">
					<div class="text-center text-gray-400 dark:text-gray-500">
						<p class="text-4xl mb-3">🔗</p>
						<p class="font-medium">{t('graph.noData')}</p>
						<p class="text-sm mt-1">{t('graph.noDataHint')}</p>
					</div>
				</div>
			{/if}
		</div>

		{#if detailPanelVisible && detailData}
			<div
				class="w-80 border-l border-gray-200 dark:border-gray-700 p-4 overflow-y-auto flex-shrink-0"
			>
				<div class="flex items-center justify-between mb-4">
					<h3 class="font-semibold text-gray-800 dark:text-gray-100">
						{detailType === 'node'
							? t('graph.selectNode')
							: t('graph.selectEdge')} Detail
					</h3>
					<button
						class="text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 cursor-pointer"
						onclick={() => graphStore.hideDetail()}>✕</button
					>
				</div>
				<div class="space-y-3">
					{#each Object.entries(detailData) as [key, value] (key)}
						{#if key !== 'properties'}
							<div class="text-sm">
								<span
									class="text-gray-500 dark:text-gray-400 block text-xs uppercase tracking-wide"
									>{key}</span
								>
								<span
									class="text-gray-800 dark:text-gray-200 font-mono text-xs break-all"
									>{String(value)}</span
								>
							</div>
						{/if}
					{/each}
				{#if detailData.properties && Object.keys(detailData.properties).length > 0}
					<div
						class="mt-4 pt-4 border-t border-gray-200 dark:border-gray-700"
					>
						<h4
							class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-2"
						>
							{t('common.properties')}
						</h4>
						{#each Object.entries(detailData.properties) as [k, v] (k)}
							<div class="text-sm mb-2">
								<span class="text-gray-500 dark:text-gray-400 block text-xs"
									>{k}</span
								>
								<span
									class="text-gray-800 dark:text-gray-200 font-mono text-xs break-all"
									>{typeof v === 'object'
										? JSON.stringify(v)
										: String(v)}</span
								>
							</div>
						{/each}
					</div>
				{/if}
				{#if detailType === 'node' && failedNodes.has(detailData.id)}
					<div class="mt-4 pt-4 border-t border-gray-200 dark:border-gray-700">
						<button
							class="w-full px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer"
							onclick={() => retryExpand(detailData.id)}
						>
							Retry Expand
						</button>
					</div>
				{/if}
				</div>
			</div>
		{/if}
	</div>
</div>
