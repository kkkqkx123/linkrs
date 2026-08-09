<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { get } from 'svelte/store';
  import { consoleStore } from '$stores/console';
  import { formatExecutionTime, formatRowCount, formatCellValue } from '$utils/parseData';
  import { exportToCSV, exportToJSON } from '$utils/export';

  let editorContent = $state('');
  let isExecuting = $state(false);
  let currentResult = $state<any>(null);
  let executionTime = $state(0);
  let error = $state<any>(null);
  let activeView = $state<'table' | 'json' | 'graph'>('table');
  let history = $state<Array<any>>([]);
  let favorites = $state<Array<any>>([]);
  let historyOpen = $state(false);
  let favoritesOpen = $state(false);
  let saveModalOpen = $state(false);
  let favoriteName = $state('');
  let saveModalError = $state('');

  onMount(() => {
    const unsub = consoleStore.subscribe(s => {
      editorContent = s.editorContent;
      isExecuting = s.isExecuting;
      currentResult = s.currentResult;
      executionTime = s.executionTime;
      error = s.error;
      activeView = s.activeView;
      history = s.history;
      favorites = s.favorites;
    });
    return unsub;
  });

  function handleKeydown(e: KeyboardEvent) {
    if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
      e.preventDefault();
      consoleStore.executeQuery();
    }
  }

  function handleExecute() {
    consoleStore.setEditorContent(editorContent);
    consoleStore.executeQuery();
  }

  function handleSaveFavorite() {
    if (!favoriteName.trim()) {
      saveModalError = get(t)('common.name') + ' is required';
      return;
    }
    const result = consoleStore.addToFavorites(favoriteName, editorContent);
    if (result.success) {
      saveModalOpen = false;
      favoriteName = '';
      saveModalError = '';
    } else {
      saveModalError = result.error || 'Failed to save';
    }
  }
</script>

<div class="flex flex-col h-full gap-4 animate-fade-in">
  <!-- Header -->
  <div class="bg-white dark:bg-[#1C2333] rounded-lg shadow-sm px-5 py-3">
    <h2 class="text-lg font-semibold text-gray-800 dark:text-gray-100 flex items-center gap-2">
      <span>⌨</span> {$t('console.title')}
    </h2>
  </div>

  <!-- Editor Section -->
  <div class="bg-white dark:bg-[#1C2333] rounded-lg shadow-sm flex flex-col">
    <div class="p-4 pb-2">
      <textarea
        class="w-full h-32 px-3 py-2 border border-gray-300 dark:border-gray-600 rounded text-sm font-mono focus:outline-none focus:border-blue-500 resize-y bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200 transition-colors"
        placeholder="{$t('console.queryPlaceholder')} {$t('console.executeHint')}"
        bind:value={editorContent}
        onkeydown={handleKeydown}
      ></textarea>
    </div>
    <div class="px-4 pb-3 flex items-center gap-2">
      <button
        class="px-4 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded transition-colors disabled:opacity-50 cursor-pointer"
        onclick={handleExecute}
        disabled={isExecuting || !editorContent.trim()}
      >
        {isExecuting ? $t('console.executing') : $t('console.execute')}
      </button>
      <button class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded transition-colors cursor-pointer" onclick={() => { consoleStore.setEditorContent(''); consoleStore.clearResult(); }}>
        {$t('console.clear')}
      </button>
      <div class="flex-1"></div>
      <button
        class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded transition-colors cursor-pointer"
        onclick={() => historyOpen = !historyOpen}
      >
        📋 {$t('console.history')} ({history.length})
      </button>
      <button
        class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded transition-colors cursor-pointer"
        onclick={() => favoritesOpen = !favoritesOpen}
      >
        ⭐ {$t('console.favorites')} ({favorites.length})
      </button>
      <button
        class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded transition-colors cursor-pointer"
        onclick={() => { favoriteName = ''; saveModalError = ''; saveModalOpen = true; }}
        disabled={!editorContent.trim()}
      >
        💾 {$t('common.save')}
      </button>
    </div>
  </div>

  <!-- Result Section -->
  <div class="flex-1 bg-white dark:bg-[#1C2333] rounded-lg shadow-sm overflow-hidden flex flex-col">
    {#if isExecuting}
      <div class="flex items-center justify-center flex-1">
        <div class="text-center">
          <div class="inline-block w-8 h-8 border-3 border-blue-500 border-t-transparent rounded-full animate-spin"></div>
          <p class="mt-2 text-sm text-gray-500 dark:text-gray-400">{$t('common.loading')}</p>
        </div>
      </div>
    {:else if error}
      <div class="m-4 p-4 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 rounded">
        <p class="text-red-700 dark:text-red-400 font-medium text-sm">{error.code}</p>
        <p class="text-red-600 dark:text-red-300 text-sm mt-1">{error.message}</p>
      </div>
    {:else if currentResult}
      <div class="px-4 py-2 bg-gray-50 dark:bg-gray-800/50 border-b border-gray-200 dark:border-gray-700 flex items-center gap-2 text-sm text-gray-500 dark:text-gray-400">
        <span>⏱ {$t('console.time')}: {formatExecutionTime(executionTime)}</span>
        <span>|</span>
        <span>{formatRowCount(currentResult.rowCount)}</span>
        <div class="flex-1"></div>
        <div class="flex gap-1">
          {#each ['table', 'json', 'graph'] as view}
            <button
              class="px-2 py-0.5 text-xs rounded cursor-pointer {activeView === view ? 'bg-blue-100 dark:bg-blue-900/30 text-blue-600 dark:text-blue-400' : 'text-gray-500 dark:text-gray-400 hover:text-gray-700 dark:hover:text-gray-300'}"
              onclick={() => { activeView = view as 'table' | 'json' | 'graph'; consoleStore.setActiveView(view as 'table' | 'json' | 'graph'); }}
            >
              {view === 'table' ? '📊 ' + $t('console.viewTable') : view === 'json' ? '{ } ' + $t('console.viewJson') : '🔗 ' + $t('console.viewGraph')}
            </button>
          {/each}
        </div>
        <div class="h-4 w-px bg-gray-300 dark:bg-gray-600"></div>
        <button class="text-blue-500 dark:text-blue-400 hover:text-blue-700 dark:hover:text-blue-300 text-xs cursor-pointer" onclick={() => exportToCSV(currentResult)}>CSV</button>
        <button class="text-blue-500 dark:text-blue-400 hover:text-blue-700 dark:hover:text-blue-300 text-xs cursor-pointer" onclick={() => exportToJSON(currentResult)}>JSON</button>
      </div>
      <div class="flex-1 overflow-auto p-4">
        {#if activeView === 'table'}
          <div class="overflow-x-auto">
            <table class="w-full text-sm border-collapse">
              <thead>
                <tr class="bg-gray-50 dark:bg-gray-800/50">
                  {#each currentResult.columns as col}
                    <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700 whitespace-nowrap">{col}</th>
                  {/each}
                </tr>
              </thead>
              <tbody>
                {#each currentResult.rows as row}
                  <tr class="hover:bg-gray-50 dark:hover:bg-gray-800/30 even:bg-gray-50/50 dark:even:bg-gray-800/20">
                    {#each row as cell}
                      <td class="px-3 py-1.5 border-b border-gray-100 dark:border-gray-700/50 text-gray-700 dark:text-gray-300 max-w-xs truncate">{formatCellValue(cell)}</td>
                    {/each}
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {:else if activeView === 'json'}
          <pre class="text-xs font-mono bg-gray-50 dark:bg-gray-800/50 p-4 rounded border border-gray-200 dark:border-gray-700 overflow-auto max-h-96 text-gray-700 dark:text-gray-300">{JSON.stringify(currentResult, null, 2)}</pre>
        {:else}
          <div class="flex items-center justify-center h-48 text-gray-400 text-sm">{$t('console.viewGraph')} - {$t('graph.noData')}</div>
        {/if}
      </div>
    {:else}
      <div class="flex items-center justify-center flex-1 text-gray-400 dark:text-gray-500 text-sm">{$t('console.noResult')}</div>
    {/if}
  </div>
</div>

<!-- History Panel -->
{#if historyOpen}
  <div class="fixed inset-0 z-50 flex justify-end">
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="absolute inset-0 bg-black/20" onclick={() => historyOpen = false}></div>
    <div class="relative w-96 bg-white dark:bg-[#1C2333] shadow-lg h-full overflow-y-auto">
      <div class="p-4 border-b border-gray-200 dark:border-gray-700 flex items-center justify-between">
        <h3 class="font-semibold text-gray-800 dark:text-gray-100">{$t('console.history')}</h3>
        <button class="text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 cursor-pointer text-lg" onclick={() => historyOpen = false}>✕</button>
      </div>
      <div class="p-4">
        {#if history.length === 0}
          <p class="text-gray-400 dark:text-gray-500 text-sm text-center py-4">{$t('console.noResult')}</p>
        {:else}
          {#each history as item}
            <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
            <div class="mb-3 p-3 border border-gray-200 dark:border-gray-700 rounded hover:bg-gray-50 dark:hover:bg-gray-700/30 cursor-pointer">
              <p class="text-xs font-mono text-gray-700 dark:text-gray-300 truncate mb-1">{item.query}</p>
              <div class="flex items-center gap-2 text-xs text-gray-400">
                <span class={item.success ? 'text-green-500' : 'text-red-500'}>{item.success ? '✓' : '✗'}</span>
                <span>{item.executionTime}ms</span>
                <span>{item.rowCount} {$t('console.rows')}</span>
              </div>
            </div>
          {/each}
          {#if history.length > 0}
            <button class="w-full text-center text-sm text-red-500 hover:text-red-700 py-2 cursor-pointer" onclick={() => consoleStore.clearHistory()}>
              {$t('common.delete')} {$t('console.history')}
            </button>
          {/if}
        {/if}
      </div>
    </div>
  </div>
{/if}

<!-- Favorites Panel -->
{#if favoritesOpen}
  <div class="fixed inset-0 z-50 flex justify-end">
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="absolute inset-0 bg-black/20" onclick={() => favoritesOpen = false}></div>
    <div class="relative w-96 bg-white dark:bg-[#1C2333] shadow-lg h-full overflow-y-auto">
      <div class="p-4 border-b border-gray-200 dark:border-gray-700 flex items-center justify-between">
        <h3 class="font-semibold text-gray-800 dark:text-gray-100">{$t('console.favorites')}</h3>
        <button class="text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 cursor-pointer text-lg" onclick={() => favoritesOpen = false}>✕</button>
      </div>
      <div class="p-4">
        {#if favorites.length === 0}
          <p class="text-gray-400 dark:text-gray-500 text-sm text-center py-4">{$t('console.noResult')}</p>
        {:else}
          {#each favorites as fav}
            <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
            <div class="mb-3 p-3 border border-gray-200 dark:border-gray-700 rounded hover:bg-gray-50 dark:hover:bg-gray-700/30 cursor-pointer">
              <p class="text-sm font-medium text-gray-800 dark:text-gray-200 mb-1">{fav.name}</p>
              <p class="text-xs font-mono text-gray-500 dark:text-gray-400 truncate">{fav.query}</p>
              <button
                class="mt-1 text-xs text-red-400 hover:text-red-600 cursor-pointer"
                onclick={(e) => { e.stopPropagation(); consoleStore.removeFromFavorites(fav.id); }}
              >
                {$t('common.delete')}
              </button>
            </div>
          {/each}
        {/if}
      </div>
    </div>
  </div>
{/if}

<!-- Save Favorite Modal -->
{#if saveModalOpen}
  <div class="fixed inset-0 z-50 flex items-center justify-center">
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="absolute inset-0 bg-black/20" onclick={() => saveModalOpen = false}></div>
    <div class="relative bg-white dark:bg-[#1C2333] rounded-lg shadow-lg p-6 w-96">
      <h3 class="font-semibold text-gray-800 dark:text-gray-100 mb-4">{$t('console.saveFavorite')}</h3>
      {#if saveModalError}
        <div class="mb-3 p-2 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 rounded text-red-600 dark:text-red-400 text-xs">{saveModalError}</div>
      {/if}
      <div class="mb-4">
        <label for="favorite-name" class="block text-sm text-gray-600 dark:text-gray-400 mb-1">{$t('common.name')}</label>
        <input
          id="favorite-name"
          type="text"
          bind:value={favoriteName}
          class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded text-sm focus:outline-none focus:border-blue-500 bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
          placeholder="{$t('console.favoriteNamePlaceholder')}"
        />
      </div>
      <div class="flex justify-end gap-2">
        <button class="px-4 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer" onclick={() => saveModalOpen = false}>
          {$t('common.cancel')}
        </button>
        <button class="px-4 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer" onclick={handleSaveFavorite}>
          {$t('common.save')}
        </button>
      </div>
    </div>
  </div>
{/if}