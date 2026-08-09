<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { t } from 'svelte-i18n';
  import { graphStore } from '$stores/graph';
  import { getLayoutOptions, applyLayout } from '$utils/graphLayout';
  import { convertToCytoscapeElements, generateCytoscapeStyle } from '$utils/cytoscapeConfig';
  import type cytoscape from 'cytoscape';

  let layout = $state('force');
  let graphData = $state<any>(null);
  let detailPanelVisible = $state(false);
  let detailData = $state<any>(null);
  let detailType = $state<string | null>(null);
  let nodeStyles = $state<Record<string, any>>({});
  let edgeStyles = $state<Record<string, any>>({});
  let cyInstance = $state<cytoscape.Core | null>(null);
  let containerEl = $state<HTMLDivElement>();
  let cyInitialized = $state(false);

  const layoutOptions = getLayoutOptions();

  onMount(() => {
    const unsub = graphStore.subscribe(s => {
      layout = s.layout;
      graphData = s.graphData;
      detailPanelVisible = s.detailPanelVisible;
      detailData = s.detailData;
      detailType = s.detailType;
      nodeStyles = s.nodeStyles;
      edgeStyles = s.edgeStyles;
    });
    return unsub;
  });

  onDestroy(() => {
    if (cyInstance) {
      cyInstance.destroy();
      cyInstance = null;
    }
  });

  $effect(() => {
    if (!containerEl || !graphData) return;
    if (!cyInitialized) {
      initCytoscape();
    } else {
      updateCytoscape();
    }
  });

  async function initCytoscape() {
    const cytoscape = (await import('cytoscape')).default;
    const dagre = (await import('cytoscape-dagre')).default;
    cytoscape.use(dagre);

    if (cyInstance) {
      cyInstance.destroy();
    }

    const elements = convertToCytoscapeElements(graphData);
    const styleConfig = {
      nodes: Object.fromEntries(
        Object.entries(nodeStyles).map(([k, v]) => [k, { color: v.color, size: v.size, labelProperty: v.labelProperty }])
      ),
      edges: Object.fromEntries(
        Object.entries(edgeStyles).map(([k, v]) => [k, { color: v.color, width: v.width, labelProperty: v.labelProperty }])
      ),
    };
    const styles = generateCytoscapeStyle(styleConfig);

    const cy = cytoscape({
      container: containerEl,
      elements,
      style: styles,
      layout: { name: 'preset' },
      minZoom: 0.1,
      maxZoom: 10,
      wheelSensitivity: 0.3,
    });

    cy.on('tap', 'node', (evt) => {
      const node = evt.target;
      const data = node.data();
      graphStore.showDetail({
        id: data.id,
        tag: data._tag || 'unknown',
        properties: Object.fromEntries(
          Object.entries(data).filter(([k]) => !['id', 'label', '_tag'].includes(k))
        ),
      }, 'node');
    });

    cy.on('tap', 'edge', (evt) => {
      const edge = evt.target;
      const data = edge.data();
      graphStore.showDetail({
        id: data.id,
        type: data._type || 'unknown',
        source: data.source,
        target: data.target,
        rank: data._rank || 0,
        properties: Object.fromEntries(
          Object.entries(data).filter(([k]) => !['id', 'label', '_type', '_rank', 'source', 'target'].includes(k))
        ),
      }, 'edge');
    });

    cy.on('tap', (evt) => {
      if (evt.target === cy) {
        graphStore.clearSelection();
      }
    });

    cyInstance = cy;
    cyInitialized = true;

    applyLayout(cy, layout as any);
  }

  function updateCytoscape() {
    if (!cyInstance || !graphData) return;
    const elements = convertToCytoscapeElements(graphData);
    cyInstance.json({ elements });
    const styleConfig = {
      nodes: Object.fromEntries(
        Object.entries(nodeStyles).map(([k, v]) => [k, { color: v.color, size: v.size, labelProperty: v.labelProperty }])
      ),
      edges: Object.fromEntries(
        Object.entries(edgeStyles).map(([k, v]) => [k, { color: v.color, width: v.width, labelProperty: v.labelProperty }])
      ),
    };
    const styles = generateCytoscapeStyle(styleConfig);
    cyInstance.style(styles);
    applyLayout(cyInstance, layout as any);
  }

  function handleLayoutChange(e: Event) {
    const val = (e.target as HTMLSelectElement).value as any;
    graphStore.setLayout(val);
    if (cyInstance) {
      applyLayout(cyInstance, val);
    }
  }

  function handleClearGraph() {
    graphStore.clearGraphData();
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
</script>

<div class="flex flex-col h-full gap-4">
  <div class="bg-white rounded-lg shadow-sm px-5 py-3 flex items-center justify-between">
    <h2 class="text-lg font-semibold text-gray-800 flex items-center gap-2">
      <span>🔗</span> {$t('graph.title')}
    </h2>
    <div class="flex items-center gap-3">
      <button
        class="px-3 py-1.5 border border-gray-300 bg-white hover:bg-gray-50 text-gray-700 text-sm rounded cursor-pointer"
        onclick={handleFitToScreen}
        disabled={!graphData}
      >
        {$t('graph.fit')}
      </button>
      <button
        class="px-3 py-1.5 border border-gray-300 bg-white hover:bg-gray-50 text-gray-700 text-sm rounded cursor-pointer"
        onclick={handleResetZoom}
        disabled={!graphData}
      >
        {$t('graph.reset')}
      </button>
      <select
        class="px-3 py-1.5 border border-gray-300 rounded text-sm bg-white focus:outline-none focus:border-blue-500"
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

  <div class="flex-1 bg-white rounded-lg shadow-sm flex overflow-hidden">
    <div class="flex-1 relative">
      {#if graphData}
        <div bind:this={containerEl} class="absolute inset-0" style="min-height: 400px;"></div>
      {:else}
        <div class="absolute inset-0 flex items-center justify-center">
          <div class="text-center text-gray-400">
            <p class="text-4xl mb-3">🔗</p>
            <p class="font-medium">{$t('graph.noData')}</p>
            <p class="text-sm mt-1">{$t('graph.noDataHint')}</p>
          </div>
        </div>
      {/if}
    </div>

    {#if detailPanelVisible && detailData}
      <div class="w-80 border-l border-gray-200 p-4 overflow-y-auto flex-shrink-0">
        <div class="flex items-center justify-between mb-4">
          <h3 class="font-semibold text-gray-800">{detailType === 'node' ? $t('graph.selectNode') : $t('graph.selectEdge')} Detail</h3>
          <button class="text-gray-400 hover:text-gray-600 cursor-pointer" onclick={() => graphStore.hideDetail()}>✕</button>
        </div>
        <div class="space-y-3">
          {#each Object.entries(detailData) as [key, value]}
            {#if key !== 'properties'}
              <div class="text-sm">
                <span class="text-gray-500 block text-xs uppercase tracking-wide">{key}</span>
                <span class="text-gray-800 font-mono text-xs break-all">{String(value)}</span>
              </div>
            {/if}
          {/each}
          {#if detailData.properties && Object.keys(detailData.properties).length > 0}
            <div class="mt-4 pt-4 border-t border-gray-200">
              <h4 class="text-sm font-medium text-gray-700 mb-2">{$t('graph.properties')}</h4>
              {#each Object.entries(detailData.properties) as [k, v]}
                <div class="text-sm mb-2">
                  <span class="text-gray-500 block text-xs">{k}</span>
                  <span class="text-gray-800 font-mono text-xs break-all">{String(v)}</span>
                </div>
              {/each}
            </div>
          {/if}
        </div>
      </div>
    {/if}
  </div>
</div>