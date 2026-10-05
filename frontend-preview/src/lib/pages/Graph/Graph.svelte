<script lang="ts">
  import { onMount } from 'svelte';
  import { SvelteSet } from 'svelte/reactivity';
  import { get } from 'svelte/store';
  import { t } from '$i18n';
  import { graphStore, type EdgeDetail, type NodeDetail } from '$stores/graph';
  import { schemaStore } from '$stores/schema';
  import { notificationStore } from '$stores/notification';
  import { graphService } from '$services/graph';
  import { theme } from '$stores/theme';
  import { getLayoutOptions } from '$utils/graphLayout';
  import { makeEdgeId } from '$utils/cytoscapeConfig';
  import CytoscapeCanvas from '$components/common/CytoscapeCanvas.svelte';
  import type { GraphData, GraphStyleConfig, LayoutType } from '$types/graph';
  import type cytoscape from 'cytoscape';

  let layout = $state<LayoutType>('force');
  let graphData = $state<GraphData | null>(null);
  let nodeLabelFields = $state<Record<string, string[]>>({});
  let edgeLabelFields = $state<Record<string, string[]>>({});
  let detailPanelVisible = $state(false);
  let detailData = $state<NodeDetail | EdgeDetail | null>(null);
  let detailType = $state<'node' | 'edge' | null>(null);
  let nodeStyles = $state<Record<string, { color: string; size: 'small' | 'medium' | 'large'; labelProperty: string }>>({});
  let edgeStyles = $state<Record<string, { color: string; width: 'thin' | 'medium' | 'thick'; labelProperty: string }>>({});
  let isDark = $state(false);
  let storeZoom = $state(1);
  let stylePanelOpen = $state(false);
  let cyInstance = $state<cytoscape.Core | null>(null);
  let relayoutToken = $state(0);
  let isExpanding = $state(false);
  const expandedNodes = new SvelteSet<string>();

  const layoutOptions = getLayoutOptions();

  function buildStyleConfig(): GraphStyleConfig {
    return {
      nodes: Object.fromEntries(
        Object.entries(nodeStyles).map(([k, v]) => [k, { color: v.color, size: v.size, labelProperty: v.labelProperty }])
      ),
      edges: Object.fromEntries(
        Object.entries(edgeStyles).map(([k, v]) => [k, { color: v.color, width: v.width, labelProperty: v.labelProperty }])
      ),
    };
  }

  const styleConfig = $derived(buildStyleConfig());

  onMount(() => {
    const unsubGraph = graphStore.subscribe(s => {
      layout = s.layout;
      graphData = s.graphData;
      detailPanelVisible = s.detailPanelVisible;
      detailData = s.detailData;
      detailType = s.detailType;
      nodeStyles = s.nodeStyles;
      edgeStyles = s.edgeStyles;
      storeZoom = s.zoom;
      if (s.graphData) {
        const nFields: Record<string, Set<string>> = {};
        const eFields: Record<string, Set<string>> = {};
        for (const n of s.graphData.nodes) {
          (nFields[n.tag] ??= new Set());
          for (const k of Object.keys(n.properties)) nFields[n.tag].add(k);
        }
        for (const e of s.graphData.edges) {
          (eFields[e.type] ??= new Set());
          for (const k of Object.keys(e.properties)) eFields[e.type].add(k);
        }
        nodeLabelFields = Object.fromEntries(Object.entries(nFields).map(([k, v]) => [k, [...v]]));
        edgeLabelFields = Object.fromEntries(Object.entries(eFields).map(([k, v]) => [k, [...v]]));
      } else {
        nodeLabelFields = {};
        edgeLabelFields = {};
      }
    });
    const unsubTheme = theme.subscribe(v => {
      isDark = v === 'dark';
    });
    return () => { unsubGraph(); unsubTheme(); };
  });

  function handleNodeTap(data: { id: string; _tag?: string; label?: string; props?: Record<string, unknown> }) {
    graphStore.selectNode(data.id);
    const detail: NodeDetail = { id: data.id, tag: data._tag || 'unknown', properties: data.props ?? {} };
    graphStore.showDetail(detail, 'node');
    void expandNode(data.id);
  }

  function handleEdgeTap(data: { id: string; source: string; target: string; _type?: string; _rank?: number; props?: Record<string, unknown> }) {
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
      notificationStore.warning('Select a space before expanding neighbors');
      return;
    }
    expandedNodes.add(id);
    isExpanding = true;
    try {
      const neighbors = await graphService.vertices.getNeighbors(id, space);
      const nodes = neighbors.map((n) => ({ id: n.vid, tag: n.tag, properties: n.properties }));
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
      notificationStore.error('Failed to load neighbors', err instanceof Error ? err.message : undefined);
    } finally {
      isExpanding = false;
    }
  }

  function handleLayoutChange(e: Event) {
    const val = (e.target as HTMLSelectElement).value as LayoutType;
    graphStore.setLayout(val);
    layout = val;
  }

  function handleClearGraph() {
    graphStore.clearGraphData();
    graphStore.hideDetail();
    expandedNodes.clear();
    if (cyInstance) {
      cyInstance.elements().remove();
    }
  }

  function handleFitToScreen() {
    cyInstance?.fit(undefined, 30);
  }

  function handleResetZoom() {
    cyInstance?.zoom(1);
    cyInstance?.center();
  }

  function handleExportPng() {
    if (!cyInstance) return;
    const png = cyInstance.png({ full: true, bg: isDark ? '#111827' : '#ffffff' });
    const link = document.createElement('a');
    link.href = png;
    link.download = 'graph.png';
    link.click();
  }

  function handleExportJson() {
    if (!graphData) return;
    const blob = new Blob([JSON.stringify(graphData, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = 'graph.json';
    link.click();
    URL.revokeObjectURL(url);
  }
</script>

<div class="flex flex-col h-full gap-4">
  <div class="bg-white dark:bg-[#1C2333] rounded-lg shadow-sm px-5 py-3 flex items-center justify-between">
    <h2 class="text-lg font-semibold text-gray-800 dark:text-gray-100 flex items-center gap-2">
      <span>🔗</span> {$t('graph.title')}
      {#if graphData}
        <span class="text-xs font-normal text-gray-500 dark:text-gray-400">{graphData.nodes.length} nodes / {graphData.edges.length} edges</span>
      {/if}
      {#if isExpanding}
        <span class="text-xs font-normal text-blue-500 dark:text-blue-400">Expanding…</span>
      {/if}
    </h2>
    <div class="flex items-center gap-3">
      <button
        class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer"
        onclick={handleFitToScreen}
        disabled={!graphData}
      >
        {$t('graph.fit')}
      </button>
      <button
        class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer"
        onclick={handleResetZoom}
        disabled={!graphData}
      >
        {$t('graph.reset')}
      </button>
      <button
        class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer"
        onclick={() => stylePanelOpen = !stylePanelOpen}
        disabled={!graphData}
      >
        Style
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
        value={layout}
        onchange={handleLayoutChange}
      >
        {#each layoutOptions as opt (opt.value)}
          <option value={opt.value}>{$t(opt.labelKey)}</option>
        {/each}
      </select>
      <button
        class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer disabled:opacity-50"
        onclick={handleClearGraph}
        disabled={!graphData}
      >
        {$t('common.clear')}
      </button>
    </div>
  </div>

  {#if stylePanelOpen && graphData}
    <div class="bg-white dark:bg-[#1C2333] rounded-lg shadow-sm px-5 py-3 grid grid-cols-2 gap-4 max-h-48 overflow-y-auto">
      <div>
        <h4 class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">Nodes</h4>
        {#each Object.entries(nodeStyles) as [tag, style] (tag)}
          <div class="flex items-center gap-2 mb-1 text-sm">
            <input type="color" value={style.color} oninput={(e) => graphStore.setNodeStyle(tag, { color: (e.target as HTMLInputElement).value })} class="w-8 h-6 cursor-pointer" />
            <span class="text-gray-700 dark:text-gray-300 font-mono text-xs">{tag}</span>
            <select
              class="ml-auto px-1 py-0.5 border border-gray-300 dark:border-gray-600 rounded text-xs bg-white dark:bg-[#1C2333] text-gray-700 dark:text-gray-300 max-w-28 truncate"
              value={style.labelProperty}
              onchange={(e) => graphStore.setNodeStyle(tag, { labelProperty: (e.target as HTMLSelectElement).value })}
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
        <h4 class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">Edges</h4>
        {#each Object.entries(edgeStyles) as [type, style] (type)}
          <div class="flex items-center gap-2 mb-1 text-sm">
            <input type="color" value={style.color} oninput={(e) => graphStore.setEdgeStyle(type, { color: (e.target as HTMLInputElement).value })} class="w-8 h-6 cursor-pointer" />
            <span class="text-gray-700 dark:text-gray-300 font-mono text-xs">{type}</span>
            <select
              class="ml-auto px-1 py-0.5 border border-gray-300 dark:border-gray-600 rounded text-xs bg-white dark:bg-[#1C2333] text-gray-700 dark:text-gray-300 max-w-28 truncate"
              value={style.labelProperty}
              onchange={(e) => graphStore.setEdgeStyle(type, { labelProperty: (e.target as HTMLSelectElement).value })}
            >
              <option value="type">type</option>
              {#each edgeLabelFields[type] ?? [] as field (field)}
                <option value={field}>{field}</option>
              {/each}
            </select>
          </div>
        {/each}
      </div>
    </div>
  {/if}

  <div class="flex-1 bg-white dark:bg-[#1C2333] rounded-lg shadow-sm flex overflow-hidden">
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
          onNodeTap={handleNodeTap}
          onEdgeTap={handleEdgeTap}
          onBackgroundTap={handleBackgroundTap}
          onZoom={(z) => graphStore.setZoom(z)}
        />
      {:else}
        <div class="absolute inset-0 flex items-center justify-center">
          <div class="text-center text-gray-400 dark:text-gray-500">
            <p class="text-4xl mb-3">🔗</p>
            <p class="font-medium">{$t('graph.noData')}</p>
            <p class="text-sm mt-1">{$t('graph.noDataHint')}</p>
          </div>
        </div>
      {/if}
    </div>

    {#if detailPanelVisible && detailData}
      <div class="w-80 border-l border-gray-200 dark:border-gray-700 p-4 overflow-y-auto flex-shrink-0">
        <div class="flex items-center justify-between mb-4">
          <h3 class="font-semibold text-gray-800 dark:text-gray-100">{detailType === 'node' ? $t('graph.selectNode') : $t('graph.selectEdge')} Detail</h3>
          <button class="text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 cursor-pointer" onclick={() => graphStore.hideDetail()}>✕</button>
        </div>
        <div class="space-y-3">
          {#each Object.entries(detailData) as [key, value] (key)}
            {#if key !== 'properties'}
              <div class="text-sm">
                <span class="text-gray-500 dark:text-gray-400 block text-xs uppercase tracking-wide">{key}</span>
                <span class="text-gray-800 dark:text-gray-200 font-mono text-xs break-all">{String(value)}</span>
              </div>
            {/if}
          {/each}
          {#if detailData.properties && Object.keys(detailData.properties).length > 0}
            <div class="mt-4 pt-4 border-t border-gray-200 dark:border-gray-700">
              <h4 class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">{$t('common.properties')}</h4>
              {#each Object.entries(detailData.properties) as [k, v] (k)}
                <div class="text-sm mb-2">
                  <span class="text-gray-500 dark:text-gray-400 block text-xs">{k}</span>
                  <span class="text-gray-800 dark:text-gray-200 font-mono text-xs break-all">{typeof v === 'object' ? JSON.stringify(v) : String(v)}</span>
                </div>
              {/each}
            </div>
          {/if}
        </div>
      </div>
    {/if}
  </div>
</div>
