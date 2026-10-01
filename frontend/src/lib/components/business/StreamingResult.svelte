<script lang="ts">
  import { t } from 'svelte-i18n';
  import VirtualTable from '$components/common/VirtualTable.svelte';
  import CytoscapeCanvas from '$components/common/CytoscapeCanvas.svelte';
  import type { StreamCardState, StreamState } from '$stores/console';
  import type { QueryResult } from '$types/query';
  import { formatExecutionTime, formatRowCount } from '$utils/parseData';
  import { exportStreamViaServer } from '$utils/export';
  import { queryResultToGraph } from '$utils/cytoscapeConfig';
  import { STREAM_JSON_PREVIEW_LIMIT } from '$utils/virtualWindow';

  const GRAPH_PREFIX_LIMIT = 5000;

  let {
    stream,
    activeView,
    isDark = false,
    onViewChange,
    onCancel,
    onOpenInGraph,
  }: {
    stream: StreamState;
    activeView: 'table' | 'json' | 'graph';
    isDark?: boolean;
    onViewChange: (view: 'table' | 'json' | 'graph') => void;
    onCancel: () => void;
    onOpenInGraph: (result: QueryResult) => void;
  } = $props();

  let previewActive = $state(false);
  let exportError = $state<string | null>(null);

  /** Server-side export re-executes the card's statement; failures leave no file. */
  async function handleServerExport(query: string, format: 'csv' | 'jsonl') {
    exportError = null;
    try {
      await exportStreamViaServer(query, format);
    } catch (error) {
      exportError = error instanceof Error ? error.message : 'Server export failed';
    }
  }

  interface CardModel {
    indexed: { index: number; row: Record<string, unknown> }[];
    dense: Record<string, unknown>[];
  }

  function cardModel(card: StreamCardState): CardModel {
    const indexed: { index: number; row: Record<string, unknown> }[] = [];
    const dense: Record<string, unknown>[] = [];
    const rows = card.rows;
    for (let i = 0; i < rows.length; i += 1) {
      const row = rows[i];
      if (row === undefined) continue;
      indexed.push({ index: i, row });
      dense.push(row);
    }
    return { indexed, dense };
  }

  function syntheticOf(card: StreamCardState, dense: Record<string, unknown>[]): QueryResult {
    return { columns: card.columns, rows: dense, rowCount: card.receivedCount };
  }

  function jsonPreviewOf(card: StreamCardState, dense: Record<string, unknown>[]): string {
    const shown = dense.slice(0, STREAM_JSON_PREVIEW_LIMIT);
    return JSON.stringify(
      { columns: card.columns, rows: shown, rowCount: card.receivedCount },
      null,
      2,
    );
  }

  let totalRows = $derived(stream.cards.reduce((sum, card) => sum + card.receivedCount, 0));

  let isActive = $derived(stream.status === 'connecting' || stream.status === 'receiving');

  let statusKey = $derived(
    stream.status === 'connecting'
      ? 'console.streamStatusConnecting'
      : stream.status === 'receiving'
        ? 'console.streamStatusReceiving'
        : stream.status === 'completed'
          ? 'console.streamStatusCompleted'
          : stream.status === 'cancelled'
            ? 'console.streamStatusCancelled'
            : 'console.streamStatusFailed',
  );

  function cardStatusKey(status: StreamCardState['status']): string {
    if (status === 'pending') return 'console.streamCardPending';
    if (status === 'receiving') return 'console.streamStatusReceiving';
    if (status === 'completed') return 'console.streamStatusCompleted';
    if (status === 'cancelled') return 'console.streamStatusCancelled';
    return 'console.streamStatusFailed';
  }
</script>

{#snippet cardBody(card: StreamCardState)}
  {@const model = cardModel(card)}
  {@const synthetic = syntheticOf(card, model.dense)}
  {#if card.error}
    <div class="mb-3 p-3 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 rounded">
      <p class="text-red-700 dark:text-red-400 font-medium text-sm">{card.error.code}</p>
      <p class="text-red-600 dark:text-red-300 text-sm mt-1">{card.error.message}</p>
      {#if card.error.code === 'ROW_LIMIT_EXCEEDED'}
        <p class="text-amber-600 dark:text-amber-400 text-xs mt-1">{$t('console.rowLimitHint')}</p>
      {/if}
    </div>
  {/if}
  {#if activeView === 'table'}
    {#if card.columns.length === 0}
      <div class="flex items-center justify-center h-48 text-gray-400 dark:text-gray-500 text-sm">{$t('console.streamWaitingColumns')}</div>
    {:else}
      <VirtualTable columns={card.columns} rows={model.indexed} />
    {/if}
  {:else if activeView === 'json'}
    {#if model.dense.length === 0}
      <div class="flex items-center justify-center h-48 text-gray-400 dark:text-gray-500 text-sm">{$t('console.noResult')}</div>
    {:else}
      {#if card.receivedCount > STREAM_JSON_PREVIEW_LIMIT}
        <p class="mb-2 text-xs text-gray-500 dark:text-gray-400">
          {$t('console.streamJsonTruncated', { values: { shown: STREAM_JSON_PREVIEW_LIMIT, total: card.receivedCount } })}
        </p>
      {/if}
      <pre class="text-xs font-mono bg-gray-50 dark:bg-gray-800/50 p-4 rounded border border-gray-200 dark:border-gray-700 overflow-auto max-h-96 text-gray-700 dark:text-gray-300">{jsonPreviewOf(card, model.dense)}</pre>
    {/if}
  {:else}
    {@const graph = queryResultToGraph({
      columns: card.columns,
      rows: model.dense.slice(0, GRAPH_PREFIX_LIMIT),
      rowCount: card.receivedCount,
    })}
    {#if graph.nodes.length > 0 || graph.edges.length > 0}
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
{/snippet}

<div class="px-4 py-2 bg-gray-50 dark:bg-gray-800/50 border-b border-gray-200 dark:border-gray-700 flex items-center gap-2 text-sm text-gray-500 dark:text-gray-400">
  <span class="inline-block w-2 h-2 rounded-full {stream.status === 'failed' ? 'bg-red-500' : stream.status === 'cancelled' ? 'bg-amber-500' : stream.status === 'completed' ? 'bg-green-500' : 'bg-blue-500 animate-pulse'}"></span>
  <span>{$t(statusKey)}</span>
  <span>|</span>
  {#if stream.executionTime > 0}
    <span>⏱ {$t('console.time')}: {formatExecutionTime(stream.executionTime)}</span>
    <span>|</span>
  {/if}
  <span>{formatRowCount(totalRows)}</span>
  {#if stream.batch}
    <span>|</span>
    <span>{$t('console.streamBatchCount', { values: { count: stream.cards.length } })}</span>
  {/if}
  {#if isActive}
    <button
      class="ml-2 px-2 py-0.5 text-xs rounded border border-red-300 dark:border-red-700 text-red-500 hover:text-red-700 cursor-pointer"
      onclick={onCancel}
    >
      {$t('console.streamCancel')}
    </button>
  {/if}
  <div class="flex-1"></div>
  <div class="flex gap-1">
    {#each ['table', 'json', 'graph'] as view}
      <button
        class="px-2 py-0.5 text-xs rounded cursor-pointer {activeView === view ? 'bg-blue-100 dark:bg-blue-900/30 text-blue-600 dark:text-blue-400' : 'text-gray-500 dark:text-gray-400 hover:text-gray-700 dark:hover:text-gray-300'}"
        onclick={() => onViewChange(view as 'table' | 'json' | 'graph')}
      >
        {view === 'table' ? '📊 ' + $t('console.viewTable') : view === 'json' ? '{ } ' + $t('console.viewJson') : '🔗 ' + $t('console.viewGraph')}
      </button>
    {/each}
  </div>
  {#if !stream.batch && stream.cards.length === 1}
    {@const only = stream.cards[0]}
    <div class="h-4 w-px bg-gray-300 dark:bg-gray-600"></div>
    <button
      class="text-blue-500 dark:text-blue-400 hover:text-blue-700 dark:hover:text-blue-300 text-xs cursor-pointer"
      title={$t('console.serverExportHint')}
      onclick={() => void handleServerExport(only.query, 'csv')}
    >CSV</button>
    <button
      class="text-blue-500 dark:text-blue-400 hover:text-blue-700 dark:hover:text-blue-300 text-xs cursor-pointer"
      title={$t('console.serverExportHint')}
      onclick={() => void handleServerExport(only.query, 'jsonl')}
    >JSONL</button>
  {/if}
</div>

<div class="flex-1 overflow-auto p-4">
  {#if exportError}
    <div class="mb-3 p-3 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 rounded flex items-start gap-2">
      <p class="flex-1 text-red-600 dark:text-red-300 text-sm">{exportError}</p>
      <button class="text-red-400 hover:text-red-600 text-sm cursor-pointer" onclick={() => { exportError = null; }} aria-label="dismiss">✕</button>
    </div>
  {/if}
  {#if stream.error && !stream.batch}
    <div class="mb-3 p-3 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 rounded">
      <p class="text-red-700 dark:text-red-400 font-medium text-sm">{stream.error.code}</p>
      <p class="text-red-600 dark:text-red-300 text-sm mt-1">{stream.error.message}</p>
    </div>
  {/if}
  {#if stream.batch}
    <div class="flex flex-col gap-3">
      {#each stream.cards as card (card.index)}
        {@const synthetic = syntheticOf(card, cardModel(card).dense)}
        <div class="border border-gray-200 dark:border-gray-700 rounded overflow-hidden">
          <div class="px-3 py-2 bg-gray-50 dark:bg-gray-800/50 flex items-center gap-2 text-xs text-gray-500 dark:text-gray-400">
            <span class={card.status === 'failed' ? 'text-red-500' : card.status === 'cancelled' ? 'text-amber-500' : card.status === 'completed' ? 'text-green-500' : 'text-blue-500'}>{card.status === 'failed' ? '✗' : card.status === 'completed' ? '✓' : card.status === 'cancelled' ? '⊘' : '…'}</span>
            <span class="text-gray-400">#{card.index + 1}</span>
            <span class="font-mono truncate flex-1 text-gray-700 dark:text-gray-300">{card.query}</span>
            <span>{$t(cardStatusKey(card.status))}</span>
            {#if card.executionTime > 0}
              <span>{formatExecutionTime(card.executionTime)}</span>
            {/if}
            <span>{formatRowCount(card.receivedCount)}</span>
            <button
              class="text-blue-500 dark:text-blue-400 hover:text-blue-700 dark:hover:text-blue-300 text-xs cursor-pointer"
              title={$t('console.serverExportHint')}
              onclick={() => void handleServerExport(card.query, 'csv')}
            >CSV</button>
            <button
              class="text-blue-500 dark:text-blue-400 hover:text-blue-700 dark:hover:text-blue-300 text-xs cursor-pointer"
              title={$t('console.serverExportHint')}
              onclick={() => void handleServerExport(card.query, 'jsonl')}
            >JSONL</button>
          </div>
          <div class="p-3">
            {@render cardBody(card)}
          </div>
        </div>
      {/each}
    </div>
  {:else if stream.cards.length === 1}
    {@render cardBody(stream.cards[0])}
  {:else}
    <div class="flex items-center justify-center h-48 text-gray-400 dark:text-gray-500 text-sm">{$t('console.noResult')}</div>
  {/if}
</div>
