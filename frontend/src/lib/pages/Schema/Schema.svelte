<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { get } from 'svelte/store';
  import { schemaStore } from '$stores/schema';
  import { DATA_TYPE_LABELS, VID_TYPES } from '$config/constants';
  import { formatDate } from '$utils/function';
  import PageSkeleton from '$components/common/PageSkeleton.svelte';

  let activeTab = $state<'spaces' | 'tags' | 'edges' | 'indexes'>('spaces');
  let pageInitialized = $state(false);

  // Spaces
  let spaces = $state<Array<any>>([]);
  let isLoadingSpaces = $state(false);
  let currentSpace = $state<string | null>(null);
  let showCreateSpace = $state(false);
  let newSpaceName = $state('');
  let newSpaceVidType = $state('INT64');
  let newSpacePartitionNum = $state(7);
  let newSpaceReplicaFactor = $state(1);

  // Tags
  let tags = $state<Array<any>>([]);
  let isLoadingTags = $state(false);
  let showCreateTag = $state(false);
  let newTagName = $state('');
  let newTagProps = $state<Array<{ name: string; data_type: string; nullable: boolean }>>([]);

  // Edges
  let edgeTypes = $state<Array<any>>([]);
  let isLoadingEdgeTypes = $state(false);
  let showCreateEdge = $state(false);
  let newEdgeName = $state('');
  let newEdgeProps = $state<Array<{ name: string; data_type: string; nullable: boolean }>>([]);

  // Indexes
  let indexes = $state<Array<any>>([]);
  let isLoadingIndexes = $state(false);
  let showCreateIndex = $state(false);
  let newIndexName = $state('');
  let newIndexType = $state('INDEX');
  let newIndexEntityType = $state('TAG');
  let newIndexEntityName = $state('');
  let newIndexFields = $state('');

  const dataTypes = Object.values(DATA_TYPE_LABELS).filter(Boolean);

  onMount(() => {
    const unsub = schemaStore.subscribe(s => {
      spaces = s.spaces;
      isLoadingSpaces = s.isLoadingSpaces;
      currentSpace = s.currentSpace;
      tags = s.tags;
      isLoadingTags = s.isLoadingTags;
      edgeTypes = s.edgeTypes;
      isLoadingEdgeTypes = s.isLoadingEdgeTypes;
      indexes = s.indexes;
      isLoadingIndexes = s.isLoadingIndexes;
    });
    schemaStore.fetchSpaces().finally(() => { pageInitialized = true; });
    return unsub;
  });

  function selectSpace(name: string) {
    schemaStore.setCurrentSpace(name);
    if (activeTab === 'tags') schemaStore.fetchTags(name);
    if (activeTab === 'edges') schemaStore.fetchEdgeTypes(name);
    if (activeTab === 'indexes') schemaStore.fetchIndexes(name);
  }

  async function handleTabChange(tab: 'spaces' | 'tags' | 'edges' | 'indexes') {
    activeTab = tab;
    if (tab === 'tags' && currentSpace) schemaStore.fetchTags(currentSpace);
    if (tab === 'edges' && currentSpace) schemaStore.fetchEdgeTypes(currentSpace);
    if (tab === 'indexes' && currentSpace) schemaStore.fetchIndexes(currentSpace);
  }

  async function createSpace() {
    if (!newSpaceName.trim()) return;
    await schemaStore.createSpace({
      name: newSpaceName,
      vidType: newSpaceVidType as 'INT64' | 'FIXED_STRING(32)',
      partitionNum: newSpacePartitionNum,
      replicaFactor: newSpaceReplicaFactor,
    });
    showCreateSpace = false;
    newSpaceName = '';
  }

  async function deleteSpace(name: string) {
    if (confirm(get(t)('common.confirmDelete', { values: { name } }))) {
      await schemaStore.deleteSpace(name);
    }
  }

  async function createTag() {
    if (!newTagName.trim() || !currentSpace) return;
    await schemaStore.createTag(currentSpace, {
      name: newTagName,
      properties: newTagProps.filter(p => p.name.trim()),
    });
    showCreateTag = false;
    newTagName = '';
    newTagProps = [];
  }

  async function deleteTag(tagName: string) {
    if (currentSpace && confirm(get(t)('common.confirmDeleteItem', { values: { name: tagName } }))) {
      await schemaStore.deleteTag(currentSpace, tagName);
    }
  }

  async function createEdge() {
    if (!newEdgeName.trim() || !currentSpace) return;
    await schemaStore.createEdgeType(currentSpace, {
      name: newEdgeName,
      properties: newEdgeProps.filter(p => p.name.trim()),
    });
    showCreateEdge = false;
    newEdgeName = '';
    newEdgeProps = [];
  }

  async function deleteEdge(edgeName: string) {
    if (currentSpace && confirm(get(t)('common.confirmDeleteItem', { values: { name: edgeName } }))) {
      await schemaStore.deleteEdgeType(currentSpace, edgeName);
    }
  }

  async function createIndex() {
    if (!newIndexName.trim() || !newIndexEntityName.trim() || !currentSpace) return;
    await schemaStore.createIndex(currentSpace, {
      name: newIndexName,
      index_type: newIndexType,
      entity_type: newIndexEntityType,
      entity_name: newIndexEntityName,
      fields: newIndexFields.split(',').map(f => f.trim()).filter(Boolean),
    });
    showCreateIndex = false;
    newIndexName = '';
    newIndexFields = '';
  }

  async function deleteIndex(indexName: string) {
    if (currentSpace && confirm(get(t)('common.confirmDeleteItem', { values: { name: indexName } }))) {
      await schemaStore.deleteIndex(currentSpace, indexName);
    }
  }

  function addProp(props: Array<any>) {
    props.push({ name: '', data_type: 'STRING', nullable: true });
  }

  function removeProp(props: Array<any>, index: number) {
    props.splice(index, 1);
  }
</script>

{#if !pageInitialized}
  <PageSkeleton />
{:else}
<div class="flex flex-col h-full gap-4">
  <div class="bg-white dark:bg-[#1C2333] rounded-lg shadow-sm px-5 py-3">
    <h2 class="text-lg font-semibold text-gray-800 dark:text-gray-100">{$t('schema.title')}</h2>
  </div>

  <!-- Space Selector -->
  <div class="bg-white dark:bg-[#1C2333] rounded-lg shadow-sm px-5 py-3">
    <div class="flex items-center gap-4">
      <span class="text-sm font-medium text-gray-600 dark:text-gray-400">{$t('sidebar.spaces')}:</span>
      <select
        class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200 focus:outline-none focus:border-blue-500"
        value={currentSpace || ''}
        onchange={(e) => selectSpace((e.target as HTMLSelectElement).value)}
      >
        <option value="">-- {$t('common.select')} --</option>
        {#each spaces as s}
          <option value={s.name}>{s.name}</option>
        {/each}
      </select>
      <button class="px-3 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer" onclick={() => { showCreateSpace = true; }}>
        + {$t('schema.createSpace')}
      </button>
    </div>
  </div>

  <!-- Tabs -->
  <div class="bg-white dark:bg-[#1C2333] rounded-lg shadow-sm flex flex-col flex-1 overflow-hidden">
    <div class="flex border-b border-gray-200 dark:border-gray-700">
      {#each ['spaces', 'tags', 'edges', 'indexes'] as tab}
        <button
          class="px-5 py-3 text-sm font-medium cursor-pointer transition-colors {activeTab === tab ? 'text-blue-600 dark:text-blue-400 border-b-2 border-blue-500' : 'text-gray-500 dark:text-gray-400 hover:text-gray-700 dark:hover:text-gray-300'}"
          onclick={() => handleTabChange(tab as 'spaces' | 'tags' | 'edges' | 'indexes')}
          disabled={tab !== 'spaces' && !currentSpace}
        >
          {tab === 'spaces' ? $t('schema.spaces') : tab === 'tags' ? $t('schema.tags') : tab === 'edges' ? $t('schema.edges') : $t('schema.indexes')}
        </button>
      {/each}
    </div>

    <div class="flex-1 overflow-auto p-4">
      {#if activeTab === 'spaces'}
        {#if isLoadingSpaces}
          <div class="flex items-center justify-center p-8"><div class="animate-spin w-6 h-6 border-2 border-blue-500 border-t-transparent rounded-full"></div></div>
        {:else if spaces.length === 0}
          <p class="text-gray-400 dark:text-gray-500 text-center py-8">{$t('schema.noSpaces')}</p>
        {:else}
          <div class="overflow-x-auto">
            <table class="w-full text-sm border-collapse">
              <thead>
                <tr class="bg-gray-50 dark:bg-gray-800/50">
                  <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">{$t('common.name')}</th>
                  <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">VID Type</th>
                  <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">{$t('common.actions')}</th>
                </tr>
              </thead>
              <tbody>
                {#each spaces as space}
                  <tr class="hover:bg-gray-50 dark:hover:bg-gray-800/30 {currentSpace === space.name ? 'bg-blue-50 dark:bg-blue-900/20' : ''}">
                    <td class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50 font-medium text-gray-800 dark:text-gray-200">{space.name}</td>
                    <td class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50 text-gray-700 dark:text-gray-300">{space.vid_type}</td>
                    <td class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50">
                      <button class="text-red-500 hover:text-red-700 text-xs cursor-pointer" onclick={() => deleteSpace(space.name)}>{$t('common.delete')}</button>
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      {:else if activeTab === 'tags'}
        <div class="flex justify-between items-center mb-4">
          <span class="text-sm text-gray-500 dark:text-gray-400">{tags.length} {$t('schema.tags')}</span>
          <button class="px-3 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer" onclick={() => { showCreateTag = true; }}>+ {$t('schema.createTag')}</button>
        </div>
        {#if isLoadingTags}
          <div class="flex items-center justify-center p-8"><div class="animate-spin w-6 h-6 border-2 border-blue-500 border-t-transparent rounded-full"></div></div>
        {:else if tags.length === 0}
          <p class="text-gray-400 dark:text-gray-500 text-center py-8">{$t('schema.noTags')}</p>
        {:else}
          <div class="overflow-x-auto">
            <table class="w-full text-sm border-collapse">
              <thead>
                <tr class="bg-gray-50 dark:bg-gray-800/50">
                  <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">{$t('common.name')}</th>
                  <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">{$t('common.properties')}</th>
                  <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">{$t('common.actions')}</th>
                </tr>
              </thead>
              <tbody>
                {#each tags as tag}
                  <tr class="hover:bg-gray-50 dark:hover:bg-gray-800/30">
                    <td class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50 font-medium text-gray-800 dark:text-gray-200">{tag.name}</td>
                    <td class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50">
                      {#if tag.properties?.length}
                        <span class="text-xs text-gray-500 dark:text-gray-400">{tag.properties.map((p: any) => p.name).join(', ')}</span>
                      {:else}
                        <span class="text-xs text-gray-400 dark:text-gray-500">{$t('common.noProperties')}</span>
                      {/if}
                    </td>
                    <td class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50">
                      <button class="text-red-500 hover:text-red-700 text-xs cursor-pointer" onclick={() => deleteTag(tag.name)}>{$t('common.delete')}</button>
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      {:else if activeTab === 'edges'}
        <div class="flex justify-between items-center mb-4">
          <span class="text-sm text-gray-500 dark:text-gray-400">{edgeTypes.length} {$t('schema.edges')}</span>
          <button class="px-3 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer" onclick={() => { showCreateEdge = true; }}>+ {$t('schema.createEdge')}</button>
        </div>
        {#if isLoadingEdgeTypes}
          <div class="flex items-center justify-center p-8"><div class="animate-spin w-6 h-6 border-2 border-blue-500 border-t-transparent rounded-full"></div></div>
        {:else if edgeTypes.length === 0}
          <p class="text-gray-400 dark:text-gray-500 text-center py-8">{$t('schema.noEdges')}</p>
        {:else}
          <div class="overflow-x-auto">
            <table class="w-full text-sm border-collapse">
              <thead>
                <tr class="bg-gray-50 dark:bg-gray-800/50">
                  <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">{$t('common.name')}</th>
                  <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">{$t('common.properties')}</th>
                  <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">{$t('common.actions')}</th>
                </tr>
              </thead>
              <tbody>
                {#each edgeTypes as edge}
                  <tr class="hover:bg-gray-50 dark:hover:bg-gray-800/30">
                    <td class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50 font-medium text-gray-800 dark:text-gray-200">{edge.name}</td>
                    <td class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50">
                      {#if edge.properties?.length}
                        <span class="text-xs text-gray-500 dark:text-gray-400">{edge.properties.map((p: any) => p.name).join(', ')}</span>
                      {:else}
                        <span class="text-xs text-gray-400 dark:text-gray-500">{$t('common.noProperties')}</span>
                      {/if}
                    </td>
                    <td class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50">
                      <button class="text-red-500 hover:text-red-700 text-xs cursor-pointer" onclick={() => deleteEdge(edge.name)}>{$t('common.delete')}</button>
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      {:else if activeTab === 'indexes'}
        <div class="flex justify-between items-center mb-4">
          <span class="text-sm text-gray-500 dark:text-gray-400">{indexes.length} {$t('schema.indexes')}</span>
          <button class="px-3 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer" onclick={() => { showCreateIndex = true; }}>+ {$t('schema.createIndex')}</button>
        </div>
        {#if isLoadingIndexes}
          <div class="flex items-center justify-center p-8"><div class="animate-spin w-6 h-6 border-2 border-blue-500 border-t-transparent rounded-full"></div></div>
        {:else if indexes.length === 0}
          <p class="text-gray-400 dark:text-gray-500 text-center py-8">{$t('schema.noIndexes')}</p>
        {:else}
          <div class="overflow-x-auto">
            <table class="w-full text-sm border-collapse">
              <thead>
                <tr class="bg-gray-50 dark:bg-gray-800/50">
                  <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">{$t('common.name')}</th>
                  <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">{$t('common.type')}</th>
                  <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">{$t('common.entity')}</th>
                  <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">{$t('common.fields')}</th>
                  <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700">{$t('common.actions')}</th>
                </tr>
              </thead>
              <tbody>
                {#each indexes as idx}
                  <tr class="hover:bg-gray-50 dark:hover:bg-gray-800/30">
                    <td class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50 font-medium text-gray-800 dark:text-gray-200">{idx.name}</td>
                    <td class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50 text-gray-700 dark:text-gray-300">{idx.index_type}</td>
                    <td class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50 text-gray-700 dark:text-gray-300">{idx.entity_type}: {idx.entity_name}</td>
                    <td class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50 text-gray-700 dark:text-gray-300">{idx.fields?.join(', ')}</td>
                    <td class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50">
                      <button class="text-red-500 hover:text-red-700 text-xs cursor-pointer" onclick={() => deleteIndex(idx.name)}>{$t('common.delete')}</button>
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      {/if}
    </div>
  </div>
</div>
{/if}

<!-- Create Space Modal -->
{#if showCreateSpace}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div class="fixed inset-0 z-50 flex items-center justify-center" onclick={() => showCreateSpace = false}>
    <div class="absolute inset-0 bg-black/20"></div>
    <div class="relative bg-white dark:bg-[#1C2333] rounded-lg shadow-lg p-6 w-96" onclick={(e) => e.stopPropagation()}>
      <h3 class="font-semibold text-gray-800 dark:text-gray-100 mb-4">{$t('schema.createSpace')}</h3>
      <div class="space-y-3">
        <div>
          <label for="space-name" class="block text-sm text-gray-600 dark:text-gray-400 mb-1">{$t('common.name')}</label>
          <input id="space-name" type="text" bind:value={newSpaceName} class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded text-sm focus:outline-none focus:border-blue-500 bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200" placeholder="{$t('sidebar.spaces')}" />
        </div>
        <div>
          <label for="space-vid-type" class="block text-sm text-gray-600 dark:text-gray-400 mb-1">VID Type</label>
          <select id="space-vid-type" bind:value={newSpaceVidType} class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200 focus:outline-none focus:border-blue-500">
            <option value="INT64">INT64</option>
            <option value="FIXED_STRING(32)">FIXED_STRING(32)</option>
          </select>
        </div>
        <div>
          <label for="space-partition-num" class="block text-sm text-gray-600 dark:text-gray-400 mb-1">{$t('schema.partitionNum')}</label>
          <input id="space-partition-num" type="number" bind:value={newSpacePartitionNum} class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded text-sm focus:outline-none focus:border-blue-500 bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200" />
        </div>
        <div>
          <label for="space-replica-factor" class="block text-sm text-gray-600 dark:text-gray-400 mb-1">{$t('schema.replicaFactor')}</label>
          <input id="space-replica-factor" type="number" bind:value={newSpaceReplicaFactor} class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded text-sm focus:outline-none focus:border-blue-500 bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200" />
        </div>
      </div>
      <div class="flex justify-end gap-2 mt-4">
        <button class="px-4 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer" onclick={() => showCreateSpace = false}>{$t('common.cancel')}</button>
        <button class="px-4 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer" onclick={createSpace}>{$t('common.create')}</button>
      </div>
    </div>
  </div>
{/if}

<!-- Create Tag Modal -->
{#if showCreateTag}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div class="fixed inset-0 z-50 flex items-center justify-center" onclick={() => showCreateTag = false}>
    <div class="absolute inset-0 bg-black/20"></div>
    <div class="relative bg-white dark:bg-[#1C2333] rounded-lg shadow-lg p-6 w-96 max-h-[80vh] overflow-y-auto" onclick={(e) => e.stopPropagation()}>
      <h3 class="font-semibold text-gray-800 dark:text-gray-100 mb-4">{$t('schema.createTag')}</h3>
      <div class="space-y-3">
        <div>
          <label for="tag-name" class="block text-sm text-gray-600 dark:text-gray-400 mb-1">{$t('common.name')}</label>
          <input id="tag-name" type="text" bind:value={newTagName} class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded text-sm focus:outline-none focus:border-blue-500 bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200" placeholder="{$t('schema.tags')}" />
        </div>
        <div>
          <!-- svelte-ignore a11y_label_has_associated_control -->
          <label class="block text-sm text-gray-600 dark:text-gray-400 mb-1">{$t('common.properties')}</label>
          {#each newTagProps as prop, i}
            <div class="flex gap-2 mb-2 items-start">
              <input type="text" bind:value={prop.name} class="flex-1 px-2 py-1.5 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200" placeholder="{$t('common.name')}" />
              <select bind:value={prop.data_type} class="px-2 py-1.5 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200">
                {#each dataTypes as dt}
                  <option value={dt}>{dt}</option>
                {/each}
              </select>
              <button class="text-red-400 hover:text-red-600 cursor-pointer px-1" onclick={() => removeProp(newTagProps, i)}>✕</button>
            </div>
          {/each}
          <button class="text-blue-500 hover:text-blue-700 text-xs cursor-pointer" onclick={() => addProp(newTagProps)}>+ {$t('common.addProperty')}</button>
        </div>
      </div>
      <div class="flex justify-end gap-2 mt-4">
        <button class="px-4 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer" onclick={() => showCreateTag = false}>{$t('common.cancel')}</button>
        <button class="px-4 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer" onclick={createTag}>{$t('common.create')}</button>
      </div>
    </div>
  </div>
{/if}

<!-- Create Edge Modal -->
{#if showCreateEdge}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div class="fixed inset-0 z-50 flex items-center justify-center" onclick={() => showCreateEdge = false}>
    <div class="absolute inset-0 bg-black/20"></div>
    <div class="relative bg-white dark:bg-[#1C2333] rounded-lg shadow-lg p-6 w-96 max-h-[80vh] overflow-y-auto" onclick={(e) => e.stopPropagation()}>
      <h3 class="font-semibold text-gray-800 dark:text-gray-100 mb-4">{$t('schema.createEdge')}</h3>
      <div class="space-y-3">
        <div>
          <label for="edge-name" class="block text-sm text-gray-600 dark:text-gray-400 mb-1">{$t('common.name')}</label>
          <input id="edge-name" type="text" bind:value={newEdgeName} class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200" placeholder="{$t('schema.edges')}" />
        </div>
        <div>
          <!-- svelte-ignore a11y_label_has_associated_control -->
          <label class="block text-sm text-gray-600 dark:text-gray-400 mb-1">{$t('common.properties')}</label>
          {#each newEdgeProps as prop, i}
            <div class="flex gap-2 mb-2">
              <input type="text" bind:value={prop.name} class="flex-1 px-2 py-1.5 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200" placeholder="{$t('common.name')}" />
              <select bind:value={prop.data_type} class="px-2 py-1.5 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200">
                {#each dataTypes as dt}
                  <option value={dt}>{dt}</option>
                {/each}
              </select>
              <button class="text-red-400 hover:text-red-600 cursor-pointer" onclick={() => removeProp(newEdgeProps, i)}>✕</button>
            </div>
          {/each}
          <button class="text-blue-500 hover:text-blue-700 text-xs cursor-pointer" onclick={() => addProp(newEdgeProps)}>+ {$t('common.addProperty')}</button>
        </div>
      </div>
      <div class="flex justify-end gap-2 mt-4">
        <button class="px-4 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer" onclick={() => showCreateEdge = false}>{$t('common.cancel')}</button>
        <button class="px-4 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer" onclick={createEdge}>{$t('common.create')}</button>
      </div>
    </div>
  </div>
{/if}

<!-- Create Index Modal -->
{#if showCreateIndex}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div role="presentation" class="fixed inset-0 z-50 flex items-center justify-center" onclick={() => showCreateIndex = false}>
    <div class="absolute inset-0 bg-black/20"></div>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="relative bg-white dark:bg-[#1C2333] rounded-lg shadow-lg p-6 w-96" onclick={(e) => e.stopPropagation()}>
      <h3 class="font-semibold text-gray-800 dark:text-gray-100 mb-4">{$t('schema.createIndex')}</h3>
      <div class="space-y-3">
        <div>
          <label for="index-name" class="block text-sm text-gray-600 dark:text-gray-400 mb-1">{$t('common.name')}</label>
          <input id="index-name" type="text" bind:value={newIndexName} class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200" placeholder="{$t('schema.indexes')}" />
        </div>
        <div>
          <label for="index-entity-type" class="block text-sm text-gray-600 dark:text-gray-400 mb-1">{$t('common.entityType')}</label>
          <select id="index-entity-type" bind:value={newIndexEntityType} class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200">
            <option value="TAG">TAG</option>
            <option value="EDGE">EDGE</option>
          </select>
        </div>
        <div>
          <label for="index-entity-name" class="block text-sm text-gray-600 dark:text-gray-400 mb-1">{$t('common.entityName')}</label>
          <input id="index-entity-name" type="text" bind:value={newIndexEntityName} class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200" placeholder="{$t('schema.tags')} / {$t('schema.edges')}" />
        </div>
        <div>
          <label for="index-fields" class="block text-sm text-gray-600 dark:text-gray-400 mb-1">{$t('common.fields')}</label>
          <input id="index-fields" type="text" bind:value={newIndexFields} class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200" placeholder="field1, field2" />
        </div>
      </div>
      <div class="flex justify-end gap-2 mt-4">
        <button class="px-4 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer" onclick={() => showCreateIndex = false}>{$t('common.cancel')}</button>
        <button class="px-4 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer" onclick={createIndex}>{$t('common.create')}</button>
      </div>
    </div>
  </div>
{/if}