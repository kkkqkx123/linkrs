<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { t } from 'svelte-i18n';
  import { graphStore, type EdgeDetail, type NodeDetail } from '$stores/graph';
  import { theme } from '$stores/theme';
  import { getLayoutOptions, applyLayout } from '$utils/graphLayout';
  import { convertToCytoscapeElements, generateCytoscapeStyle } from '$utils/cytoscapeConfig';
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
  let containerEl = $state<HTMLDivElement>();
  let cyInitialized = $state(false);
  let eventsBound = $state(false);

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

  onDestroy(() => {
    if (cyInstance) {
      cyInstance.destroy();
      cyInstance = null;
    }
    cyInitialized = false;
    eventsBound = false;
  });

  $effect(() => {
    if (!containerEl || !graphData) return;
    if (!cyInitialized) {
      void initCytoscape();
    } else {
      refreshStyle();
    }
  });

  function bindEvents(cy: cytoscape.Core) {
    if (eventsBound) return;
    cy.on('tap', 'node', (evt) => {
      const data = evt.target.data() as { id: string; _tag?: string; label?: string; props?: Record<string, unknown> };
      graphStore.selectNode(data.id);
      const detail: NodeDetail = { id: data.id, tag: data._tag || 'unknown', properties: data.props ?? {} };
      graphStore.showDetail(detail, 'node');
    });
    cy.on('tap', 'edge', (evt) => {
      const data = evt.target.data() as { id: string; source: string; target: string; _type?: string; _rank?: number; props?: Record<string, unknown> };
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
    });
    cy.on('tap', (evt) => {
      if (evt.target === cy) {
        cy.elements().unselect();
        graphStore.clearSelection();
      }
    });
    cy.on('zoom', () => {
      graphStore.setZoom(cy.zoom());
    });
    eventsBound = true;
  }

  async function initCytoscape() {
    if (!containerEl || !graphData) return;
    const cytoscape = (await import('cytoscape')).default;

    if (cyInstance) {
      cyInstance.destroy();
      cyInstance = null;
      eventsBound = false;
    }

    const styleConfig = buildStyleConfig();
    const cy = cytoscape({
      container: containerEl,
      elements: convertToCytoscapeElements(graphData, styleConfig),
      style: generateCytoscapeStyle(styleConfig, isDark),
      layout: { name: 'preset' },
      minZoom: 0.1,
      maxZoom: 10,
      wheelSensitivity: 0.3,
    });

    bindEvents(cy);
    cyInstance = cy;
    cyInitialized = true;
    applyLayout(cy, layout, cy.elements().length);
    if (storeZoom > 0 && storeZoom !== 1) {
      cy.zoom(storeZoom);
    }
  }

  function syncElements(relayout: boolean) {
    if (!cyInstance || !graphData) return;
    const styleConfig = buildStyleConfig();
    const elements = convertToCytoscapeElements(graphData, styleConfig);
    const savedZoom = cyInstance.zoom();
    const savedPan = { ...cyInstance.pan() };
    const existingIds = new Set(cyInstance.elements().map((el) => el.id()));
    const nextIds = new Set(elements.map((el) => String((el.data as { id: string }).id)));
    cyInstance.batch(() => {
      cyInstance?.elements().filter((el) => !nextIds.has(el.id())).remove();
      const toAdd = elements.filter((el) => !existingIds.has(String((el.data as { id: string }).id)));
      if (toAdd.length > 0) cyInstance?.add(toAdd);
      for (const el of elements) {
        const id = String((el.data as { id: string }).id);
        const existing = cyInstance?.getElementById(id);
        if (existing && existing.nonempty()) {
          existing.data('label', (el.data as { label: string }).label);
        }
      }
    });
    cyInstance.zoom(savedZoom);
    cyInstance.pan(savedPan);
    if (relayout) {
      applyLayout(cyInstance, layout, cyInstance.elements().length);
    }
  }

  function refreshStyle() {
    if (!cyInstance) return;
    cyInstance.style(generateCytoscapeStyle(buildStyleConfig(), isDark));
    syncElements(false);
  }

  function handleLayoutChange(e: Event) {
    const val = (e.target as HTMLSelectElement).value as LayoutType;
    graphStore.setLayout(val);
    layout = val;
    if (cyInstance) {
      applyLayout(cyInstance, val, cyInstance.elements().length);
    }
  }

  function handleClearGraph() {
    graphStore.clearGraphData();
    graphStore.hideDetail();
    if (cyInstance) {
      cyInstance.elements().remove();
    }
  }

  function handleFitToScreen() {
    if (cyInstance) {
      cyInstance.fit(undefined, 30);
    }
  }

  function handleResetZoom() {
    if (cyInstance) {
      cyInstance.zoom(1);
      cyInstance.center();
    }
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
        {#each layoutOptions as opt}
          <option value={opt.value}>{opt.label}</option>
        {/each}
      </select>
      <button
        class="px-3 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer disabled:opacity-50"
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
        {#each Object.entries(nodeStyles) as [tag, style]}
          <div class="flex items-center gap-2 mb-1 text-sm">
            <input type="color" value={style.color} oninput={(e) => graphStore.setNodeStyle(tag, { color: (e.target as HTMLInputElement).value })} class="w-8 h-6 cursor-pointer" />
            <span class="text-gray-700 dark:text-gray-300 font-mono text-xs">{tag}</span>
            <select
              class="ml-auto px-1 py-0.5 border border-gray-300 dark:border-gray-600 rounded text-xs bg-white dark:bg-[#1C2333] text-gray-700 dark:text-gray-300 max-w-28 truncate"
              value={style.labelProperty}
              onchange={(e) => graphStore.setNodeStyle(tag, { labelProperty: (e.target as HTMLSelectElement).value })}
            >
              <option value="id">id</option>
              {#each nodeLabelFields[tag] ?? [] as field}
                <option value={field}>{field}</option>
              {/each}
            </select>
          </div>
        {/each}
      </div>
      <div>
        <h4 class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">Edges</h4>
        {#each Object.entries(edgeStyles) as [type, style]}
          <div class="flex items-center gap-2 mb-1 text-sm">
            <input type="color" value={style.color} oninput={(e) => graphStore.setEdgeStyle(type, { color: (e.target as HTMLInputElement).value })} class="w-8 h-6 cursor-pointer" />
            <span class="text-gray-700 dark:text-gray-300 font-mono text-xs">{type}</span>
            <select
              class="ml-auto px-1 py-0.5 border border-gray-300 dark:border-gray-600 rounded text-xs bg-white dark:bg-[#1C2333] text-gray-700 dark:text-gray-300 max-w-28 truncate"
              value={style.labelProperty}
              onchange={(e) => graphStore.setEdgeStyle(type, { labelProperty: (e.target as HTMLSelectElement).value })}
            >
              <option value="type">type</option>
              {#each edgeLabelFields[type] ?? [] as field}
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
        <div bind:this={containerEl} class="absolute inset-0" style="min-height: 400px;"></div>
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
          {#each Object.entries(detailData) as [key, value]}
            {#if key !== 'properties'}
              <div class="text-sm">
                <span class="text-gray-500 dark:text-gray-400 block text-xs uppercase tracking-wide">{key}</span>
                <span class="text-gray-800 dark:text-gray-200 font-mono text-xs break-all">{String(value)}</span>
              </div>
            {/if}
          {/each}
          {#if detailData.properties && Object.keys(detailData.properties).length > 0}
            <div class="mt-4 pt-4 border-t border-gray-200 dark:border-gray-700">
              <h4 class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">{$t('graph.properties')}</h4>
              {#each Object.entries(detailData.properties) as [k, v]}
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
