<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { dataBrowserStore } from '$stores/dataBrowser';
  import { schemaStore } from '$stores/schema';
  import { dataBrowserService } from '$services/dataBrowser';
  import { formatCellValue } from '$utils/parseData';
  import PageSkeleton from '$components/common/PageSkeleton.svelte';

  let currentSpace = $state<string | null>(null);
  let tags = $state<Array<any>>([]);
  let edgeTypes = $state<Array<any>>([]);

  let activeTab = $state<'vertices' | 'edges'>('vertices');
  let selectedTag = $state<string | null>(null);
  let selectedEdgeType = $state<string | null>(null);
  let vertices = $state<Array<any>>([]);
  let edges = $state<Array<any>>([]);
  let vertexTotal = $state(0);
  let edgeTotal = $state(0);
  let vertexPage = $state(1);
  let edgePage = $state(1);
  let vertexPageSize = $state(50);
  let edgePageSize = $state(50);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let statistics = $state<any>(null);
  let filterPanelVisible = $state(false);
  let detailModalVisible = $state(false);
  let detailData = $state<any>(null);
  let detailType = $state<string | null>(null);

  let vertexProperties = $state<string[]>([]);
  let edgeProperties = $state<string[]>([]);
  let pageInitialized = $state(false);

  onMount(() => {
    const unsub1 = schemaStore.subscribe(s => {
      currentSpace = s.currentSpace;
      tags = s.tags;
      edgeTypes = s.edgeTypes;
    });
    const unsub2 = dataBrowserStore.subscribe(s => {
      activeTab = s.activeTab;
      selectedTag = s.selectedTag;
      selectedEdgeType = s.selectedEdgeType;
      vertices = s.vertices;
      edges = s.edges;
      vertexTotal = s.vertexTotal;
      edgeTotal = s.edgeTotal;
      vertexPage = s.vertexPage;
      edgePage = s.edgePage;
      vertexPageSize = s.vertexPageSize;
      edgePageSize = s.edgePageSize;
      loading = s.loading;
      error = s.error;
      statistics = s.statistics;
      filterPanelVisible = s.filterPanelVisible;
      detailModalVisible = s.detailModalVisible;
      detailData = s.detailData;
      detailType = s.detailType;
    });
    if (currentSpace) schemaStore.fetchTags(currentSpace);
    pageInitialized = true;
    return () => { unsub1(); unsub2(); };
  });

  async function loadVertices() {
    if (!currentSpace || !selectedTag) return;
    dataBrowserStore.setLoading(true);
    dataBrowserStore.setError(null);
    try {
      const response = await dataBrowserService.getVertices(currentSpace, selectedTag, vertexPage, vertexPageSize, { field: 'id', order: 'asc' }, { conditions: [], logic: 'AND' });
      dataBrowserStore.setVertices(response.data, response.total);
      if (response.data.length > 0) vertexProperties = Object.keys(response.data[0].properties);
    } catch (err) {
      dataBrowserStore.setError(err instanceof Error ? err.message : 'Failed to load vertices');
    } finally {
      dataBrowserStore.setLoading(false);
    }
  }

  async function loadEdges() {
    if (!currentSpace || !selectedEdgeType) return;
    dataBrowserStore.setLoading(true);
    dataBrowserStore.setError(null);
    try {
      const response = await dataBrowserService.getEdges(currentSpace, selectedEdgeType, edgePage, edgePageSize, { field: 'id', order: 'asc' }, { conditions: [], logic: 'AND' });
      dataBrowserStore.setEdges(response.data, response.total);
      if (response.data.length > 0) edgeProperties = Object.keys(response.data[0].properties);
    } catch (err) {
      dataBrowserStore.setError(err instanceof Error ? err.message : 'Failed to load edges');
    } finally {
      dataBrowserStore.setLoading(false);
    }
  }

  async function loadStatistics() {
    if (!currentSpace) return;
    try {
      const stats = await dataBrowserService.getStatistics(currentSpace);
      dataBrowserStore.setStatistics(stats);
    } catch (err) { console.error('Failed to load statistics:', err); }
  }

  function handleTagChange(e: Event) {
    const val = (e.target as HTMLSelectElement).value || null;
    dataBrowserStore.setSelectedTag(val);
    if (val && currentSpace) loadVertices();
  }

  function handleEdgeTypeChange(e: Event) {
    const val = (e.target as HTMLSelectElement).value || null;
    dataBrowserStore.setSelectedEdgeType(val);
    if (val && currentSpace) loadEdges();
  }

  function handleTabChange(tab: 'vertices' | 'edges') {
    dataBrowserStore.setActiveTab(tab);
    if (tab === 'vertices' && currentSpace) { schemaStore.fetchTags(currentSpace); if (selectedTag) loadVertices(); }
    if (tab === 'edges' && currentSpace) { schemaStore.fetchEdgeTypes(currentSpace); if (selectedEdgeType) loadEdges(); }
  }

  function showDetail(data: any, type: 'vertex' | 'edge') {
    dataBrowserStore.showDetail(data, type);
  }
</script>

{#if !pageInitialized}
  <PageSkeleton />
{:else if currentSpace}
  <div class="flex flex-col h-full gap-4">
    <div class="bg-white dark:bg-[#1C2333] rounded-lg shadow-sm px-5 py-3 flex items-center justify-between">
      <h2 class="text-lg font-semibold text-gray-800 dark:text-gray-100">📋 {$t('dataBrowser.title')}</h2>
      <div class="flex gap-2">
        <button class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer" onclick={loadStatistics}>
          🔄 {$t('common.refresh')}
        </button>
        <button
          class="px-3 py-1.5 border rounded text-sm cursor-pointer {filterPanelVisible ? 'bg-blue-50 dark:bg-blue-900/30 border-blue-300 dark:border-blue-700 text-blue-600 dark:text-blue-400' : 'border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300'}"
          onclick={() => dataBrowserStore.toggleFilterPanel()}
        >
          🔍 {$t('dataBrowser.filter')}
        </button>
      </div>
    </div>

    {#if error}
      <div class="bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 rounded p-3 text-red-600 dark:text-red-400 text-sm">{error}</div>
    {/if}

    <div class="flex-1 bg-white dark:bg-[#1C2333] rounded-lg shadow-sm overflow-hidden flex">
      <div class="flex-1 flex flex-col overflow-hidden">
        <div class="flex border-b border-gray-200 dark:border-gray-700">
          <button class="px-5 py-3 text-sm font-medium cursor-pointer {activeTab === 'vertices' ? 'text-blue-600 dark:text-blue-400 border-b-2 border-blue-500' : 'text-gray-500 dark:text-gray-400 hover:text-gray-700 dark:hover:text-gray-300'}" onclick={() => handleTabChange('vertices')}>
            📦 {$t('dataBrowser.vertices')}
          </button>
          <button class="px-5 py-3 text-sm font-medium cursor-pointer {activeTab === 'edges' ? 'text-blue-600 dark:text-blue-400 border-b-2 border-blue-500' : 'text-gray-500 dark:text-gray-400 hover:text-gray-700 dark:hover:text-gray-300'}" onclick={() => handleTabChange('edges')}>
            ↔ {$t('dataBrowser.edges')}
          </button>
        </div>

        <div class="p-4 flex-1 overflow-auto">
          {#if activeTab === 'vertices'}
            <div class="mb-4">
              <select class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200" value={selectedTag || ''} onchange={handleTagChange}>
                <option value="">-- {$t('dataBrowser.selectTag')} --</option>
                {#each tags as tag}
                  <option value={tag.name}>{tag.name}</option>
                {/each}
              </select>
            </div>

            {#if loading}
              <div class="flex items-center justify-center p-8"><div class="animate-spin w-6 h-6 border-2 border-blue-500 border-t-transparent rounded-full"></div></div>
            {:else if vertices.length > 0}
              <div class="overflow-x-auto">
                <table class="w-full text-sm border-collapse">
                  <thead>
                    <tr class="bg-gray-50 dark:bg-gray-800/50">
                      <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">ID</th>
                      <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">{$t('schema.tags')}</th>
                      {#each vertexProperties as prop}
                        <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">{prop}</th>
                      {/each}
                      <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">{$t('common.actions')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {#each vertices as v}
                      <tr class="hover:bg-gray-50 dark:hover:bg-gray-800/30">
                        <td class="px-3 py-1.5 border-b border-gray-100 dark:border-gray-700/50 font-mono text-xs text-gray-800 dark:text-gray-200">{v.id}</td>
                        <td class="px-3 py-1.5 border-b border-gray-100 dark:border-gray-700/50 text-gray-700 dark:text-gray-300">{v.tag}</td>
                        {#each vertexProperties as prop}
                          <td class="px-3 py-1.5 border-b border-gray-100 dark:border-gray-700/50 max-w-40 truncate text-gray-700 dark:text-gray-300">{formatCellValue(v.properties[prop])}</td>
                        {/each}
                        <td class="px-3 py-1.5 border-b border-gray-100 dark:border-gray-700/50">
                          <button class="text-blue-500 dark:text-blue-400 hover:text-blue-700 dark:hover:text-blue-300 text-xs cursor-pointer" onclick={() => showDetail(v, 'vertex')}>{$t('dataBrowser.viewDetail')}</button>
                        </td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </div>
              <div class="flex items-center justify-between mt-4 text-sm text-gray-500 dark:text-gray-400">
                <span>{$t('dataBrowser.total')}: {vertexTotal} {$t('dataBrowser.items')}</span>
                <div class="flex gap-2">
                  <button
                    class="px-3 py-1 border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 cursor-pointer disabled:opacity-50"
                    disabled={vertexPage <= 1}
                    onclick={() => { dataBrowserStore.setVertexPage(vertexPage - 1); loadVertices(); }}
                  >{$t('common.prev')}</button>
                  <span class="px-3 py-1 text-gray-600 dark:text-gray-400">{$t('common.page')} {vertexPage}</span>
                  <button
                    class="px-3 py-1 border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 cursor-pointer disabled:opacity-50"
                    disabled={vertexPage * vertexPageSize >= vertexTotal}
                    onclick={() => { dataBrowserStore.setVertexPage(vertexPage + 1); loadVertices(); }}
                  >{$t('common.next')}</button>
                </div>
              </div>
            {:else if selectedTag}
              <p class="text-gray-400 dark:text-gray-500 text-center py-8">{$t('schema.noTags')}</p>
            {:else}
              <p class="text-gray-400 dark:text-gray-500 text-center py-8">{$t('dataBrowser.selectTag')} {$t('common.loading')}</p>
            {/if}
          {:else}
            <div class="mb-4">
              <select class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200" value={selectedEdgeType || ''} onchange={handleEdgeTypeChange}>
                <option value="">-- {$t('dataBrowser.selectEdgeType')} --</option>
                {#each edgeTypes as et}
                  <option value={et.name}>{et.name}</option>
                {/each}
              </select>
            </div>

            {#if loading}
              <div class="flex items-center justify-center p-8"><div class="animate-spin w-6 h-6 border-2 border-blue-500 border-t-transparent rounded-full"></div></div>
            {:else if edges.length > 0}
              <div class="overflow-x-auto">
                <table class="w-full text-sm border-collapse">
                  <thead>
                    <tr class="bg-gray-50 dark:bg-gray-800/50">
                      <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">ID</th>
                      <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">{$t('common.type')}</th>
                      <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">Source</th>
                      <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">Target</th>
                      <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">Rank</th>
                      {#each edgeProperties as prop}
                        <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">{prop}</th>
                      {/each}
                      <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">{$t('common.actions')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {#each edges as e}
                      <tr class="hover:bg-gray-50 dark:hover:bg-gray-800/30">
                        <td class="px-3 py-1.5 border-b border-gray-100 dark:border-gray-700/50 font-mono text-xs text-gray-800 dark:text-gray-200">{e.id}</td>
                        <td class="px-3 py-1.5 border-b border-gray-100 dark:border-gray-700/50 text-gray-700 dark:text-gray-300">{e.type}</td>
                        <td class="px-3 py-1.5 border-b border-gray-100 dark:border-gray-700/50 font-mono text-xs text-gray-800 dark:text-gray-200">{e.src}</td>
                        <td class="px-3 py-1.5 border-b border-gray-100 dark:border-gray-700/50 font-mono text-xs text-gray-800 dark:text-gray-200">{e.dst}</td>
                        <td class="px-3 py-1.5 border-b border-gray-100 dark:border-gray-700/50 text-gray-700 dark:text-gray-300">{e.rank}</td>
                        {#each edgeProperties as prop}
                          <td class="px-3 py-1.5 border-b border-gray-100 dark:border-gray-700/50 max-w-40 truncate text-gray-700 dark:text-gray-300">{formatCellValue(e.properties[prop])}</td>
                        {/each}
                        <td class="px-3 py-1.5 border-b border-gray-100 dark:border-gray-700/50">
                          <button class="text-blue-500 dark:text-blue-400 hover:text-blue-700 dark:hover:text-blue-300 text-xs cursor-pointer" onclick={() => showDetail(e, 'edge')}>{$t('dataBrowser.viewDetail')}</button>
                        </td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </div>
              <div class="flex items-center justify-between mt-4 text-sm text-gray-500 dark:text-gray-400">
                <span>{$t('dataBrowser.total')}: {edgeTotal} {$t('dataBrowser.items')}</span>
                <div class="flex gap-2">
                  <button
                    class="px-3 py-1 border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 cursor-pointer disabled:opacity-50"
                    disabled={edgePage <= 1}
                    onclick={() => { dataBrowserStore.setEdgePage(edgePage - 1); loadEdges(); }}
                  >{$t('common.prev')}</button>
                  <span class="px-3 py-1 text-gray-600 dark:text-gray-400">{$t('common.page')} {edgePage}</span>
                  <button
                    class="px-3 py-1 border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 cursor-pointer disabled:opacity-50"
                    disabled={edgePage * edgePageSize >= edgeTotal}
                    onclick={() => { dataBrowserStore.setEdgePage(edgePage + 1); loadEdges(); }}
                  >{$t('common.next')}</button>
                </div>
              </div>
            {:else if selectedEdgeType}
              <p class="text-gray-400 dark:text-gray-500 text-center py-8">{$t('schema.noEdges')}</p>
            {:else}
              <p class="text-gray-400 dark:text-gray-500 text-center py-8">{$t('dataBrowser.selectEdgeType')} {$t('common.loading')}</p>
            {/if}
          {/if}
        </div>
      </div>

      <!-- Statistics Panel -->
      <div class="w-64 border-l border-gray-200 dark:border-gray-700 p-4 bg-gray-50 dark:bg-gray-800/20 overflow-y-auto">
        <h3 class="font-semibold text-gray-800 dark:text-gray-100 mb-3 text-sm">{$t('sidebar.stats')}</h3>
        {#if statistics}
          <div class="space-y-2 text-sm">
            <div class="flex justify-between"><span class="text-gray-500 dark:text-gray-400">{$t('dataBrowser.vertices')}:</span><span class="font-medium text-gray-800 dark:text-gray-200">{statistics.totalVertices ?? '-'}</span></div>
            <div class="flex justify-between"><span class="text-gray-500 dark:text-gray-400">{$t('dataBrowser.edges')}:</span><span class="font-medium text-gray-800 dark:text-gray-200">{statistics.totalEdges ?? '-'}</span></div>
            <div class="flex justify-between"><span class="text-gray-500 dark:text-gray-400">{$t('schema.tags')}:</span><span class="font-medium text-gray-800 dark:text-gray-200">{statistics.tagCount ?? '-'}</span></div>
            <div class="flex justify-between"><span class="text-gray-500 dark:text-gray-400">{$t('schema.edges')}:</span><span class="font-medium text-gray-800 dark:text-gray-200">{statistics.edgeTypeCount ?? '-'}</span></div>
          </div>
        {:else}
          <p class="text-gray-400 dark:text-gray-500 text-xs">{$t('common.refresh')} {$t('common.loading')}</p>
        {/if}
      </div>
    </div>
  </div>
{:else}
  <div class="bg-white dark:bg-[#1C2333] rounded-lg shadow-sm p-8 text-center">
    <p class="text-gray-500 dark:text-gray-400">{$t('common.select')} {$t('sidebar.spaces')} {$t('common.loading')}</p>
  </div>
{/if}

<!-- Detail Modal -->
{#if detailModalVisible && detailData}
  <div class="fixed inset-0 z-50 flex items-center justify-center">
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div role="presentation" class="absolute inset-0 bg-black/20" onclick={() => dataBrowserStore.hideDetail()}></div>
    <div class="relative bg-white dark:bg-[#1C2333] rounded-lg shadow-lg p-6 w-96 max-h-[80vh] overflow-y-auto">
      <div class="flex items-center justify-between mb-4">
        <h3 class="font-semibold text-gray-800 dark:text-gray-100">{detailType === 'vertex' ? $t('dataBrowser.vertices') : $t('dataBrowser.edges')} {$t('common.detail')}</h3>
        <button class="text-gray-400 dark:text-gray-500 hover:text-gray-600 dark:hover:text-gray-300 cursor-pointer" onclick={() => dataBrowserStore.hideDetail()}>✕</button>
      </div>
      <div class="space-y-2">
        {#each Object.entries(detailData) as [key, value]}
          {#if key !== 'properties'}
            <div class="text-sm"><span class="text-gray-500 dark:text-gray-400">{key}:</span> <span class="ml-1 text-gray-800 dark:text-gray-200">{String(value)}</span></div>
          {/if}
        {/each}
        {#if detailData.properties}
          <div class="mt-4">
            <h4 class="text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">{$t('common.properties')}</h4>
            {#each Object.entries(detailData.properties) as [k, v]}
              <div class="text-sm ml-2"><span class="text-gray-500 dark:text-gray-400">{k}:</span> <span class="ml-1 text-gray-800 dark:text-gray-200">{String(v)}</span></div>
            {/each}
          </div>
        {/if}
      </div>
    </div>
  </div>
{/if}