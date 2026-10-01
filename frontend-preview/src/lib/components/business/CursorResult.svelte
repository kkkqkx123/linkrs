<script lang="ts">
  import { t } from 'svelte-i18n';
  import VirtualTable from '$components/common/VirtualTable.svelte';
  import CytoscapeCanvas from '$components/common/CytoscapeCanvas.svelte';
  import type { CursorState } from '$stores/console';
  import type { QueryResult } from '$types/query';
  import { formatRowCount } from '$utils/parseData';
  import { exportStreamViaServer } from '$utils/export';
  import { queryResultToGraph } from '$utils/cytoscapeConfig';
  import { STREAM_JSON_PREVIEW_LIMIT } from '$utils/virtualWindow';

  const GRAPH_PREFIX_LIMIT = 5000;

  let {
    cursor,
    activeView,
    isDark = false,
    onViewChange,
    onLoadMore,
    onClose,
    onOpenInGraph,
  }: {
    cursor: CursorState;
    activeView: 'table' | 'json' | 'graph';
    isDark?: boolean;
    onViewChange: (view: 'table' | 'json' | 'graph') => void;
    onLoadMore: () => void;
    onClose: () => void;
    onOpenInGraph: (result: QueryResult) => void;
  } = $props();

  let previewActive = $state(false);
  let exportError = $state<string | null>(null);

  let indexed = $derived.by(() => {
    const out: { index: number; row: Record<string, unknown> }[] = [];
    for (let i = 0; i < cursor.rows.length; i += 1) {
      out.push({ index: i, row: cursor.rows[i] });
    }
    return out;
  });

  let synthetic = $derived<QueryResult>({
    columns: cursor.columns,
    rows: cursor.rows,
    rowCount: cursor.receivedCount,
  });

  let graph = $derived.by(() => {
    if (activeView !== 'graph') return null;
    return queryResultToGraph({
      columns: cursor.columns,
      rows: cursor.rows.slice(0, GRAPH_PREFIX_LIMIT),
      rowCount: cursor.receivedCount,
    });
  });

  let jsonPreview = $derived.by(() => {
    const shown = cursor.rows.slice(0, STREAM_JSON_PREVIEW_LIMIT);
    return JSON.stringify(
      { columns: cursor.columns, rows: shown, rowCount: cursor.receivedCount },
      null,
      2,
    );
  });

  let loading = $derived(cursor.status === 'opening' || cursor.status === 'fetching');

  let statusKey = $derived(
    cursor.status === 'opening'
      ? 'console.cursorStatusOpening'
      : cursor.status === 'fetching'
        ? 'console.streamStatusReceiving'
        : cursor.status === 'exhausted'
          ? 'console.cursorStatusExhausted'
          : cursor.status === 'failed'
            ? 'console.streamStatusFailed'
            : 'console.cursorStatusOpen',
  );

  async function handleServerExport(format: 'csv' | 'jsonl') {
    exportError = null;
    try {
      await exportStreamViaServer(cursor.query, format);
    } catch (error) {
      exportError = error instanceof Error ? error.message : 'Server export failed';
    }
  }
</script>

<div class="px-4 py-2 bg-gray-50 dark:bg-gray-800/50 border-b border-gray-200 dark:border-gray-700 flex items-center gap-2 text-sm text-gray-500 dark:text-gray-400">
  <span class="inline-block w-2 h-2 rounded-full {cursor.status === 'failed' ? 'bg-red-500' : cursor.status === 'exhausted' ? 'bg-green-500' : 'bg-blue-500 animate-pulse'}"></span>
  <span>{$t(statusKey)}</span>
  <span>|</span>
  <span>{formatRowCount(cursor.receivedCount)}{cursor.hasMore ? '+' : ''}</span>
  {#if cursor.hasMore && cursor.status === 'open'}
    <button
      class="ml-2 px-2 py-0.5 text-xs rounded border border-blue-300 dark:border-blue-700 text-blue-500 hover:text-blue-700 cursor-pointer"
      onclick={onLoadMore}
    >
      {$t('console.cursorLoadMore')}
    </button>
  {/if}
  <button
    class="px-2 py-0.5 text-xs rounded border border-gray-300 dark:border-gray-600 text-gray-500 hover:text-gray-700 cursor-pointer"
    onclick={onClose}
  >
    {$t('console.cursorClose')}
  </button>
  <div class="flex-1"></div>
  <div class="flex gap-1">
    {#each ['table', 'json', 'graph'] as view (view)}
      <button
        class="px-2 py-0.5 text-xs rounded cursor-pointer {activeView === view ? 'bg-blue-100 dark:bg-blue-900/30 text-blue-600 dark:text-blue-400' : 'text-gray-500 dark:text-gray-400 hover:text-gray-700 dark:hover:text-gray-300'}"
        onclick={() => onViewChange(view as 'table' | 'json' | 'graph')}
      >
        {view === 'table' ? '📊 ' + $t('console.viewTable') : view === 'json' ? '{ } ' + $t('console.viewJson') : '🔗 ' + $t('console.viewGraph')}
      </button>
    {/each}
  </div>
  <div class="h-4 w-px bg-gray-300 dark:bg-gray-600"></div>
  <button
    class="text-blue-500 dark:text-blue-400 hover:text-blue-700 dark:hover:text-blue-300 text-xs cursor-pointer"
    title={$t('console.serverExportHint')}
    onclick={() => void handleServerExport('csv')}
  >CSV</button>
  <button
    class="text-blue-500 dark:text-blue-400 hover:text-blue-700 dark:hover:text-blue-300 text-xs cursor-pointer"
    title={$t('console.serverExportHint')}
    onclick={() => void handleServerExport('jsonl')}
  >JSONL</button>
</div>

<div class="flex-1 overflow-auto p-4">
  {#if exportError}
    <div class="mb-3 p-3 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 rounded flex items-start gap-2">
      <p class="flex-1 text-red-600 dark:text-red-300 text-sm">{exportError}</p>
      <button class="text-red-400 hover:text-red-600 text-sm cursor-pointer" onclick={() => { exportError = null; }} aria-label="dismiss">✕</button>
    </div>
  {/if}
  {#if cursor.error}
    <div class="mb-3 p-3 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 rounded">
      <p class="text-red-700 dark:text-red-400 font-medium text-sm">{cursor.error.code}</p>
      <p class="text-red-600 dark:text-red-300 text-sm mt-1">{cursor.error.message}</p>
    </div>
  {/if}
  {#if loading && cursor.rows.length === 0}
    <div class="flex items-center justify-center h-48 text-gray-400 dark:text-gray-500 text-sm">{$t('common.loading')}</div>
  {:else if activeView === 'table'}
    {#if cursor.columns.length === 0}
      <div class="flex items-center justify-center h-48 text-gray-400 dark:text-gray-500 text-sm">{$t('console.streamWaitingColumns')}</div>
    {:else}
      <VirtualTable columns={cursor.columns} rows={indexed} />
      {#if loading}
        <p class="mt-2 text-xs text-gray-400">{$t('common.loading')}</p>
      {/if}
    {/if}
  {:else if activeView === 'json'}
    {#if cursor.rows.length === 0}
      <div class="flex items-center justify-center h-48 text-gray-400 dark:text-gray-500 text-sm">{$t('console.noResult')}</div>
    {:else}
      {#if cursor.receivedCount > STREAM_JSON_PREVIEW_LIMIT}
        <p class="mb-2 text-xs text-gray-500 dark:text-gray-400">
          {$t('console.streamJsonTruncated', { values: { shown: STREAM_JSON_PREVIEW_LIMIT, total: cursor.receivedCount } })}
        </p>
      {/if}
      <pre class="text-xs font-mono bg-gray-50 dark:bg-gray-800/50 p-4 rounded border border-gray-200 dark:border-gray-700 overflow-auto max-h-96 text-gray-700 dark:text-gray-300">{jsonPreview}</pre>
    {/if}
  {:else}
    {#if graph && (graph.nodes.length > 0 || graph.edges.length > 0)}
      <div class="flex flex-col gap-2 text-sm text-gray-600 dark:text-gray-300">
        <div class="flex flex-col items-center gap-2">
          <p>🔗 {graph.nodes.length} nodes / {graph.edges.length} edges{#if graph.stats.truncated} (truncated){/if}</p>
          {#if graph.stats.skipped > 0}
            <p class="text-xs text-gray-400">{graph.stats.skipped} scalar cells ignored</p>
          {/if}
          <div class="flex gap-2">
            <button class="px-4 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer" onclick={() => onOpenInGraph(synthetic)}>
              Open in Graph
            </button>
            <button
              class="px-4 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer"
              onclick={() => { previewActive = !previewActive; }}
            >
              {previewActive ? 'Hide Preview' : 'Preview'}
            </button>
          </div>
        </div>
        {#if previewActive}
          <div class="h-80 rounded border border-gray-200 dark:border-gray-700 relative overflow-hidden">
            <CytoscapeCanvas data={graph} {isDark} />
          </div>
        {/if}
      </div>
    {:else}
      <div class="flex items-center justify-center h-48 text-gray-400 text-sm">{$t('console.viewGraph')} - {$t('graph.noData')}</div>
    {/if}
  {/if}
</div>
