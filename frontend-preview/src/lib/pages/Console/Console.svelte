<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { t } from '$i18n';
  import { get } from 'svelte/store';
  import { navigate } from 'svelte-routing';
  import { consoleStore, type QueryHistoryItem, type QueryFavoriteItem, type StatementResultEntry, type StreamState, type CursorState, type ResultMode, type AutoDecision } from '$stores/console';
  import { clampAutoThreshold, type ExecutionPreference } from '$utils/autoRoute';
  import { graphStore } from '$stores/graph';
  import { theme } from '$stores/theme';
  import { formatExecutionTime, formatRowCount, formatCellValue } from '$utils/parseData';
  import { queryResultToGraph } from '$utils/cytoscapeConfig';
  import CytoscapeCanvas from '$components/common/CytoscapeCanvas.svelte';
  import CypherEditor from '$components/common/CypherEditor.svelte';
  import StreamingResult from '$components/business/StreamingResult.svelte';
  import CursorResult from '$components/business/CursorResult.svelte';
  import { exportToCSV, exportToJSON } from '$utils/export';
  import { queryService } from '$services/query';
  import { formatQuery, getStreamEligibility, splitQueries } from '$utils/gql';
  import type { QueryResult, QueryError } from '$types/query';

  let editorContent = $state('');
  let isExecuting = $state(false);
  let currentResult = $state<QueryResult | null>(null);
  let results = $state<StatementResultEntry[]>([]);
  let executionTime = $state(0);
  let error = $state<QueryError | null>(null);
  let activeView = $state<'table' | 'json' | 'graph'>('table');
  let stream = $state<StreamState | null>(null);
  let cursor = $state<CursorState | null>(null);
  let resultMode = $state<ResultMode>(null);
  let executionPreference = $state<ExecutionPreference>('materialized');
  let autoStreamThreshold = $state(1000);
  let autoDecision = $state<AutoDecision | null>(null);
  let isDark = $state(false);
  let history = $state<QueryHistoryItem[]>([]);
  let favorites = $state<QueryFavoriteItem[]>([]);
  let historyOpen = $state(false);
  let favoritesOpen = $state(false);
  let saveModalOpen = $state(false);
  let favoriteName = $state('');
  let saveModalError = $state('');
  let validateMessage = $state('');
  let isValidating = $state(false);

  interface ParamRow { id: string; name: string; text: string; }

  let rowSeq = 0;
  const nextRowId = () => `param-${Date.now()}-${rowSeq++}`;

  function parseParamValue(text: string): unknown {
    const trimmed = text.trim();
    if (!trimmed) return '';
    try {
      return JSON.parse(trimmed);
    } catch {
      return text;
    }
  }

  function rowsFromRecord(record: Record<string, unknown>): ParamRow[] {
    return Object.entries(record).map(([name, value]) => ({
      id: nextRowId(),
      name,
      text: typeof value === 'string' ? value : JSON.stringify(value),
    }));
  }

  function rowsToRecord(rows: ParamRow[]): Record<string, unknown> {
    const out: Record<string, unknown> = {};
    for (const row of rows) {
      if (!row.name.trim()) continue;
      out[row.name.trim()] = parseParamValue(row.text);
    }
    return out;
  }

  let parameters = $state<ParamRow[]>([]);
  let sessionVariables = $state<ParamRow[]>([]);
  let paramsOpen = $state(false);

  /** True when a run produced more than one statement outcome. */
  let isMultiResult = $derived(results.length > 1);

  /** Primary entry backing the single-result header (first success wins). */
  let primaryEntry = $derived(results.find((e) => e.success) ?? results[0] ?? null);

  /** Human-readable stage breakdown for the header hover detail. */
  function stageDetail(entry: StatementResultEntry | null): string {
    if (!entry?.stages) return '';
    const parts = Object.entries(entry.stages).map(([k, v]) => {
      const num = typeof v === 'number' ? v : Number(v);
      return `${k}: ${Number.isFinite(num) ? num.toFixed(2) : '?'}ms`;
    });
    const trace = entry.traceId ? ` | trace: ${entry.traceId}` : '';
    return parts.join(' | ') + trace;
  }

  /** Streaming transport is holding an open or finished stream. */
  let streamingActive = $derived(
    stream !== null && stream.status !== 'idle' && resultMode === 'stream',
  );

  /** Cursor transport is holding an open cursor. */
  let cursorBusy = $derived(
    cursor !== null &&
      (cursor.status === 'opening' || cursor.status === 'fetching') &&
      resultMode === 'cursor',
  );

  /** Whether the full buffer qualifies for the streaming endpoint. */
  let streamEligibility = $derived(getStreamEligibility(editorContent));

  /** Toolbar busy state covering materialized, streaming, and cursor runs. */
  let isBusy = $derived(isExecuting || streamingActive || cursorBusy);

  let graphSummary = $derived.by(() => {
    if (!currentResult) return null;
    const parsed = queryResultToGraph(currentResult);
    return { nodes: parsed.nodes.length, edges: parsed.edges.length, skipped: parsed.stats.skipped, truncated: parsed.stats.truncated };
  });

  let graph = $derived.by(() => {
    if (!currentResult) return null;
    return queryResultToGraph(currentResult);
  });

  let previewActive = $state(false);

  function handlePreviewToggle(open: boolean) {
    previewActive = open;
  }

  onMount(() => {
    const snapshot = get(consoleStore);
    parameters = rowsFromRecord(snapshot.parameters);
    sessionVariables = rowsFromRecord(snapshot.sessionVariables);
    const unsub = consoleStore.subscribe(s => {
      editorContent = s.editorContent;
      isExecuting = s.isExecuting;
      currentResult = s.currentResult;
      results = s.results;
      executionTime = s.executionTime;
      error = s.error;
      activeView = s.activeView;
      history = s.history;
      favorites = s.favorites;
      stream = s.stream;
      cursor = s.cursor;
      resultMode = s.resultMode;
      executionPreference = s.executionPreference;
      autoStreamThreshold = s.autoStreamThreshold;
      autoDecision = s.autoDecision;
    });
    const unsubTheme = theme.subscribe(v => { isDark = v === 'dark'; });
    return () => {
      unsub();
      unsubTheme();
      if (saveTimer) clearTimeout(saveTimer);
      if (validateTimer) clearTimeout(validateTimer);
    };
  });

  onDestroy(() => {
    consoleStore.cancelStream();
    void consoleStore.closeCursor();
  });

  let saveTimer: ReturnType<typeof setTimeout> | null = null;

  // Persist editor content into the store after the user pauses typing, so the
  // draft survives reloads without writing to localStorage on every keystroke.
  $effect(() => {
    const content = editorContent;
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(() => consoleStore.setEditorContent(content), 300);
  });

  // Re-check the buffer in the background after the user pauses typing, so
  // mistakes surface without an explicit validate click. Manual validation
  // stays available for an immediate answer.
  $effect(() => {
    const content = editorContent;
    if (validateTimer) clearTimeout(validateTimer);
    if (!content.trim()) {
      validateMessage = '';
      return;
    }
    validateTimer = setTimeout(() => void autoValidate(content), 800);
  });

  // Push binding rows into the store whenever the panel edits them, so the
  // next execution carries the latest values without an explicit save step.
  $effect(() => {
    consoleStore.setParameters(rowsToRecord(parameters));
  });

  $effect(() => {
    consoleStore.setSessionVariables(rowsToRecord(sessionVariables));
  });

  /**
   * Execute the text handed over by the editor, which is either the current
   * selection, the statement under the caret, or the whole buffer.
   */
  function handleExecute(text?: string) {
    if (saveTimer) clearTimeout(saveTimer);
    if (typeof text === 'string' && text.trim()) {
      consoleStore.executeQueryByText(text);
      return;
    }
    consoleStore.setEditorContent(editorContent);
    consoleStore.executeQuery();
  }

  /** Run the full buffer through the streaming endpoint when eligible. */
  function handleStreamExecute() {
    if (saveTimer) clearTimeout(saveTimer);
    if (!streamEligibility.eligible || isBusy) return;
    consoleStore.setEditorContent(editorContent);
    void consoleStore.startStream(editorContent);
  }

  /** Page through a single statement with a server cursor. */
  function handleCursorExecute() {
    if (saveTimer) clearTimeout(saveTimer);
    if (streamEligibility.mode !== 'single' || isBusy) return;
    consoleStore.setEditorContent(editorContent);
    void consoleStore.openCursor(editorContent);
  }

  function handleCursorLoadMore() {
    void consoleStore.fetchMoreCursor();
  }

  function handleCursorClose() {
    void consoleStore.closeCursor();
  }

  function handleCursorViewChange(view: 'table' | 'json' | 'graph') {
    activeView = view;
    consoleStore.setActiveView(view);
  }

  function handleCursorOpenInGraph(result: QueryResult) {
    const parsed = queryResultToGraph(result);
    graphStore.setGraphData({ nodes: parsed.nodes, edges: parsed.edges });
    navigate('/graph');
  }

  function handleCancelStream() {
    consoleStore.cancelStream();
  }

  function handleStreamViewChange(view: 'table' | 'json' | 'graph') {
    activeView = view;
    consoleStore.setActiveView(view);
  }

  function handleStreamOpenInGraph(result: QueryResult) {
    const parsed = queryResultToGraph(result);
    graphStore.setGraphData({ nodes: parsed.nodes, edges: parsed.edges });
    navigate('/graph');
  }

  /** Parse-and-bind each statement without executing it, reporting the first problem. */
  async function handleValidate() {
    const statements = splitQueries(editorContent);
    if (statements.length === 0) return;
    isValidating = true;
    validateMessage = '';
    try {
      for (const statement of statements) {
        const outcome = await queryService.validate(statement);
        if (!outcome.valid) {
          validateMessage = outcome.message;
          break;
        }
        validateMessage = outcome.message;
      }
    } finally {
      isValidating = false;
    }
  }

  function handleFormat() {
    const formatted = formatQuery(editorContent);
    if (formatted && formatted !== editorContent) {
      editorContent = formatted;
      consoleStore.setEditorContent(formatted);
    }
  }

  let validateTimer: ReturnType<typeof setTimeout> | null = null;

  /** Quietly re-check the first problem after the user pauses typing. */
  async function autoValidate(content: string) {
    if (isValidating) return;
    const statements = splitQueries(content);
    if (statements.length === 0) return;
    try {
      for (const statement of statements) {
        const outcome = await queryService.validate(statement);
        if (content !== editorContent) return;
        if (!outcome.valid) {
          validateMessage = outcome.message;
          return;
        }
        validateMessage = outcome.message;
      }
    } catch { /* ignore background check failures */ }
  }

  function handleSaveFavorite() {
    if (!favoriteName.trim()) {
      saveModalError = get(t)('console.favoriteNameRequired');
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

  function handleOpenInGraph() {
    if (!currentResult) return;
    const parsed = queryResultToGraph(currentResult);
    graphStore.setGraphData({ nodes: parsed.nodes, edges: parsed.edges });
    navigate('/graph');
  }

  /** Load a history entry into the editor without executing it. */
  function handleLoadHistory(item: QueryHistoryItem) {
    consoleStore.loadFromHistory(item.query);
    historyOpen = false;
  }

  /** Re-run a history entry through current routing; results may differ. */
  function handleRerunHistory(item: QueryHistoryItem) {
    consoleStore.setEditorContent(item.query);
    historyOpen = false;
    void consoleStore.executeQuery();
  }

  function handleLoadFavorite(fav: QueryFavoriteItem) {
    consoleStore.loadFromFavorites(fav.query);
    favoritesOpen = false;
  }

  function handleRerunFavorite(fav: QueryFavoriteItem) {
    consoleStore.setEditorContent(fav.query);
    favoritesOpen = false;
    void consoleStore.executeQuery();
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
      <CypherEditor
        bind:value={editorContent}
        {isDark}
        placeholder="{$t('console.queryPlaceholder')} {$t('console.executeHint')}"
        onExecute={handleExecute}
        historyProvider={() => history.map(item => item.query)}
      />
    </div>
    <div class="px-4 pb-3 flex items-center gap-2 flex-wrap">
      <button
        class="px-4 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded transition-colors disabled:opacity-50 cursor-pointer"
        onclick={() => handleExecute()}
        disabled={isBusy || !editorContent.trim()}
      >
        {isExecuting ? $t('console.executing') : $t('console.execute')}
      </button>
      <button
        class="px-4 py-1.5 bg-teal-500 hover:bg-teal-600 text-white text-sm rounded transition-colors disabled:opacity-50 cursor-pointer"
        onclick={handleStreamExecute}
        disabled={isBusy || !streamEligibility.eligible}
        title={streamEligibility.mode === 'single' && parameters.length > 0 ? $t('console.streamParamsIgnored') : undefined}
      >
        {$t('console.execStream')}
      </button>
      <button
        class="px-4 py-1.5 bg-indigo-500 hover:bg-indigo-600 text-white text-sm rounded transition-colors disabled:opacity-50 cursor-pointer"
        onclick={handleCursorExecute}
        disabled={isBusy || streamEligibility.mode !== 'single'}
        title={$t('console.cursorExecuteHint')}
      >
        {$t('console.cursorExecute')}
      </button>
      {#if editorContent.trim() && !streamEligibility.eligible && streamEligibility.reason !== 'empty'}
        <span class="text-xs text-gray-400 dark:text-gray-500">
          {$t('console.streamReasonCommand')}
        </span>
      {:else if streamEligibility.mode === 'batch'}
        <span class="text-xs text-gray-400 dark:text-gray-500">
          {$t('console.streamBatchHint', { values: { count: streamEligibility.count } })}
        </span>
      {/if}
      <button
        class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded transition-colors cursor-pointer disabled:opacity-50"
        onclick={handleValidate}
        disabled={isValidating || !editorContent.trim()}
      >
        {isValidating ? $t('console.validating') : $t('console.validate')}
      </button>
      <button
        class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded transition-colors cursor-pointer disabled:opacity-50"
        onclick={handleFormat}
        disabled={!editorContent.trim()}
      >
        {$t('console.format')}
      </button>
      <select
        class="px-2 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer"
        bind:value={executionPreference}
        onchange={() => consoleStore.setExecutionPreference(executionPreference)}
        title={$t('console.execPreferenceHint')}
      >
        <option value="materialized">{$t('console.execMaterialized')}</option>
        <option value="stream">{$t('console.execStream')}</option>
        <option value="auto">{$t('console.execAuto')}</option>
      </select>
      {#if executionPreference === 'auto'}
        <label class="flex items-center gap-1 text-xs text-gray-500 dark:text-gray-400">
          {$t('console.autoThreshold')}
          <input
            type="number"
            min="1"
            max="10000000"
            step="100"
            class="w-24 px-2 py-1 border border-gray-300 dark:border-gray-600 rounded text-xs font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
            bind:value={autoStreamThreshold}
            onchange={() => consoleStore.setAutoStreamThreshold(clampAutoThreshold(autoStreamThreshold))}
          />
        </label>
      {/if}
      <button class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded transition-colors cursor-pointer" onclick={() => { consoleStore.setEditorContent(''); consoleStore.clearResult(); }}>
        {$t('common.clear')}
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
    {#if autoDecision}
      <div class="mx-4 mb-3 p-2 text-xs rounded border border-blue-200 dark:border-blue-800 bg-blue-50 dark:bg-blue-900/20 text-blue-700 dark:text-blue-300">
        {#if autoDecision.estimatedRows === null}
          {$t('console.autoDecisionUnknown', { values: { threshold: autoDecision.threshold } })}
        {:else if autoDecision.path === 'stream'}
          {$t('console.autoDecisionStream', { values: { estimated: autoDecision.estimatedRows, threshold: autoDecision.threshold } })}
        {:else}
          {$t('console.autoDecisionMaterialized', { values: { estimated: autoDecision.estimatedRows, threshold: autoDecision.threshold } })}
        {/if}
      </div>
    {/if}
    <div class="px-4 pb-3">
      <button
        class="text-xs text-gray-500 dark:text-gray-400 hover:text-gray-700 dark:hover:text-gray-200 cursor-pointer"
        onclick={() => paramsOpen = !paramsOpen}
      >
        {paramsOpen ? '▾' : '▸'} {$t('console.parameters')} ({parameters.length + sessionVariables.length})
      </button>
      {#if paramsOpen}
        <div class="mt-2 grid grid-cols-1 md:grid-cols-2 gap-3">
          <div class="border border-gray-200 dark:border-gray-700 rounded p-2">
            <p class="text-xs font-medium text-gray-600 dark:text-gray-300 mb-1">@ {$t('console.parameters')}</p>
            {#each parameters as row (row.id)}
              <div class="flex gap-1 mb-1">
                <input
                  type="text"
                  bind:value={row.name}
                  placeholder={$t('common.name')}
                  class="w-1/3 px-2 py-1 border border-gray-300 dark:border-gray-600 rounded text-xs font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
                />
                <input
                  type="text"
                  bind:value={row.text}
                  placeholder={$t('console.bindingValue')}
                  class="flex-1 px-2 py-1 border border-gray-300 dark:border-gray-600 rounded text-xs font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
                />
                <button
                  class="px-1.5 text-xs text-red-400 hover:text-red-600 cursor-pointer"
                  onclick={() => { parameters = parameters.filter(r => r.id !== row.id); }}
                  aria-label={$t('common.delete')}
                >✕</button>
              </div>
            {/each}
            <button
              class="text-xs text-blue-500 dark:text-blue-400 hover:text-blue-700 dark:hover:text-blue-300 cursor-pointer"
              onclick={() => { parameters = [...parameters, { id: nextRowId(), name: '', text: '' }]; }}
            >
              {$t('console.addBinding')}
            </button>
          </div>
          <div class="border border-gray-200 dark:border-gray-700 rounded p-2">
            <p class="text-xs font-medium text-gray-600 dark:text-gray-300 mb-1">$ {$t('console.sessionVariables')}</p>
            {#each sessionVariables as row (row.id)}
              <div class="flex gap-1 mb-1">
                <input
                  type="text"
                  bind:value={row.name}
                  placeholder={$t('common.name')}
                  class="w-1/3 px-2 py-1 border border-gray-300 dark:border-gray-600 rounded text-xs font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
                />
                <input
                  type="text"
                  bind:value={row.text}
                  placeholder={$t('console.bindingValue')}
                  class="flex-1 px-2 py-1 border border-gray-300 dark:border-gray-600 rounded text-xs font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
                />
                <button
                  class="px-1.5 text-xs text-red-400 hover:text-red-600 cursor-pointer"
                  onclick={() => { sessionVariables = sessionVariables.filter(r => r.id !== row.id); }}
                  aria-label={$t('common.delete')}
                >✕</button>
              </div>
            {/each}
            <button
              class="text-xs text-blue-500 dark:text-blue-400 hover:text-blue-700 dark:hover:text-blue-300 cursor-pointer"
              onclick={() => { sessionVariables = [...sessionVariables, { id: nextRowId(), name: '', text: '' }]; }}
            >
              {$t('console.addBinding')}
            </button>
          </div>
        </div>
        {#if (streamEligibility.mode === 'single' && !streamingActive) || (streamingActive && stream && !stream.batch)}
          <p class="mt-2 text-xs text-amber-600 dark:text-amber-400">{$t('console.streamParamsIgnored')}</p>
        {/if}
      {/if}
    </div>
    {#if validateMessage}
      <div class="mx-4 mb-3 p-2 text-xs rounded border border-gray-200 dark:border-gray-700 bg-gray-50 dark:bg-gray-800/50 text-gray-600 dark:text-gray-300">
        {validateMessage}
      </div>
    {/if}
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
    {:else if resultMode === 'stream' && stream}
      <StreamingResult
        {stream}
        {activeView}
        {isDark}
        onViewChange={handleStreamViewChange}
        onCancel={handleCancelStream}
        onOpenInGraph={handleStreamOpenInGraph}
      />
    {:else if resultMode === 'cursor' && cursor}
      <CursorResult
        {cursor}
        {activeView}
        {isDark}
        onViewChange={handleCursorViewChange}
        onLoadMore={handleCursorLoadMore}
        onClose={handleCursorClose}
        onOpenInGraph={handleCursorOpenInGraph}
      />
    {:else if error && results.length === 0}
      <div class="m-4 p-4 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 rounded">
        <p class="text-red-700 dark:text-red-400 font-medium text-sm">{error.code}</p>
        <p class="text-red-600 dark:text-red-300 text-sm mt-1">{error.message}</p>
      </div>
    {:else if isMultiResult || (results.length === 1 && !currentResult)}
      <div class="px-4 py-2 bg-gray-50 dark:bg-gray-800/50 border-b border-gray-200 dark:border-gray-700 flex items-center gap-2 text-sm text-gray-500 dark:text-gray-400">
        <span>⏱ {$t('console.time')}: {formatExecutionTime(executionTime)}</span>
        <span>|</span>
        <span>{results.length} {results.length === 1 ? 'statement' : 'statements'}</span>
      </div>
      <div class="flex-1 overflow-auto p-4 flex flex-col gap-3">
        {#each results as entry, index (index)}
          <details class="border border-gray-200 dark:border-gray-700 rounded overflow-hidden">
            <summary class="px-3 py-2 bg-gray-50 dark:bg-gray-800/50 flex items-center gap-2 text-xs text-gray-500 dark:text-gray-400 cursor-pointer">
              <span class={entry.success ? 'text-green-500' : 'text-red-500'}>{entry.success ? '✓' : '✗'}</span>
              <span class="text-gray-400">#{index + 1}</span>
              <span class="font-mono truncate flex-1 text-gray-700 dark:text-gray-300">{entry.query}</span>
              <span title={entry.stages ? stageDetail(entry) : undefined}>{formatExecutionTime(entry.executionTime)}</span>
              {#if entry.traceId}
                <span class="font-mono text-gray-400" title={entry.traceId}>⛁ {entry.traceId.slice(0, 8)}</span>
              {/if}
              {#if entry.result}
                <span>{formatRowCount(entry.result.rowCount)}</span>
                {#if entry.truncated}
                  <span class="text-amber-600 dark:text-amber-400" title={$t('console.rowLimitHint')}>⚠ {$t('console.resultTruncated')}</span>
                {/if}
              {/if}
            </summary>
            <div class="p-3">
              {#if !entry.success && entry.error}
                <div class="text-red-600 dark:text-red-400 text-xs">
                  <span class="font-medium">{entry.error.code}</span>: {entry.error.message}
                </div>
              {:else if entry.result}
                {#if entry.result.columns.length > 0}
                  <div class="overflow-x-auto">
                    <table class="w-full text-sm border-collapse">
                      <thead>
                        <tr class="bg-gray-50 dark:bg-gray-800/50">
                          {#each entry.result.columns as col (col)}
                            <th class="px-3 py-1.5 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700 whitespace-nowrap">{col}</th>
                          {/each}
                        </tr>
                      </thead>
                      <tbody>
                        {#each entry.result.rows as row, i (i)}
                          <tr class="hover:bg-gray-50 dark:hover:bg-gray-800/30 even:bg-gray-50/50 dark:even:bg-gray-800/20">
                            {#each entry.result.columns as col (col)}
                              <td class="px-3 py-1 border-b border-gray-100 dark:border-gray-700/50 text-gray-700 dark:text-gray-300 max-w-xs truncate">{formatCellValue(row[col])}</td>
                            {/each}
                          </tr>
                        {/each}
                      </tbody>
                    </table>
                  </div>
                {:else}
                  <div class="text-xs text-green-600 dark:text-green-400">OK</div>
                {/if}
              {/if}
            </div>
          </details>
        {/each}
      </div>
    {:else if currentResult}
      <div class="px-4 py-2 bg-gray-50 dark:bg-gray-800/50 border-b border-gray-200 dark:border-gray-700 flex items-center gap-2 text-sm text-gray-500 dark:text-gray-400">
        <span title={primaryEntry?.stages ? `${$t('console.stages')}: ${stageDetail(primaryEntry)}` : undefined}>⏱ {$t('console.time')}: {formatExecutionTime(executionTime)}</span>
        {#if primaryEntry?.traceId}
          <span class="font-mono text-xs text-gray-400" title={primaryEntry.traceId}>⛁ {primaryEntry.traceId.slice(0, 8)}</span>
        {/if}
        <span>|</span>
        <span>{formatRowCount(currentResult.rowCount)}</span>
        {#if currentResult.truncated}
          <span class="text-amber-600 dark:text-amber-400" title={$t('console.rowLimitHint')}>⚠ {$t('console.resultTruncated')}</span>
        {/if}
        <div class="flex-1"></div>
        <div class="flex gap-1">
          {#each ['table', 'json', 'graph'] as view (view)}
            <button
              class="px-2 py-0.5 text-xs rounded cursor-pointer {activeView === view ? 'bg-blue-100 dark:bg-blue-900/30 text-blue-600 dark:text-blue-400' : 'text-gray-500 dark:text-gray-400 hover:text-gray-700 dark:hover:text-gray-300'}"
              onclick={() => { activeView = view as 'table' | 'json' | 'graph'; consoleStore.setActiveView(view as 'table' | 'json' | 'graph'); }}
            >
              {view === 'table' ? '📊 ' + $t('console.viewTable') : view === 'json' ? '{ } ' + $t('console.viewJson') : '🔗 ' + $t('console.viewGraph')}
            </button>
          {/each}
        </div>
        <div class="h-4 w-px bg-gray-300 dark:bg-gray-600"></div>
        <button class="text-blue-500 dark:text-blue-400 hover:text-blue-700 dark:hover:text-blue-300 text-xs cursor-pointer" onclick={() => { if (currentResult) exportToCSV(currentResult); }}>CSV</button>
        <button class="text-blue-500 dark:text-blue-400 hover:text-blue-700 dark:hover:text-blue-300 text-xs cursor-pointer" onclick={() => { if (currentResult) exportToJSON(currentResult); }}>JSON</button>
      </div>
      <div class="flex-1 overflow-auto p-4">
        {#if activeView === 'table'}
          <div class="overflow-x-auto">
            <table class="w-full text-sm border-collapse">
              <thead>
                <tr class="bg-gray-50 dark:bg-gray-800/50">
                  {#each currentResult.columns as col (col)}
                    <th class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700 whitespace-nowrap">{col}</th>
                  {/each}
                </tr>
              </thead>
              <tbody>
                {#each currentResult.rows as row, i (i)}
                  <tr class="hover:bg-gray-50 dark:hover:bg-gray-800/30 even:bg-gray-50/50 dark:even:bg-gray-800/20">
                    {#each currentResult.columns as col (col)}
                      <td class="px-3 py-1.5 border-b border-gray-100 dark:border-gray-700/50 text-gray-700 dark:text-gray-300 max-w-xs truncate">{formatCellValue(row[col])}</td>
                    {/each}
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {:else if activeView === 'json'}
          <pre class="text-xs font-mono bg-gray-50 dark:bg-gray-800/50 p-4 rounded border border-gray-200 dark:border-gray-700 overflow-auto max-h-96 text-gray-700 dark:text-gray-300">{JSON.stringify(currentResult, null, 2)}</pre>
        {:else}
          {#if graphSummary && (graphSummary.nodes > 0 || graphSummary.edges > 0)}
            <div class="flex flex-col gap-2 text-sm text-gray-600 dark:text-gray-300">
              <div class="flex flex-col items-center gap-2">
                <p>🔗 {graphSummary.nodes} nodes / {graphSummary.edges} edges{#if graphSummary.truncated} (truncated){/if}</p>
                {#if graphSummary.skipped > 0}
                  <p class="text-xs text-gray-400">{graphSummary.skipped} scalar cells ignored</p>
                {/if}
                <div class="flex gap-2">
                  <button class="px-4 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer" onclick={handleOpenInGraph}>
                    Open in Graph
                  </button>
                  <button
                    class="px-4 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer"
                    onclick={() => handlePreviewToggle(!previewActive)}
                  >
                    {previewActive ? 'Hide Preview' : 'Preview'}
                  </button>
                </div>
              </div>
          {#if previewActive && graph}
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
    {:else}
      <div class="flex items-center justify-center flex-1 text-gray-400 dark:text-gray-500 text-sm">{$t('console.noResult')}</div>
    {/if}
  </div>
</div>

<!-- History Panel -->
{#if historyOpen}
  <div class="fixed inset-0 z-50 flex justify-end">
    <div class="absolute inset-0 bg-black/20" role="presentation" onclick={() => historyOpen = false}></div>
    <div class="relative w-96 bg-white dark:bg-[#1C2333] shadow-lg h-full overflow-y-auto" role="dialog" aria-labelledby="history-panel-title">
      <div id="history-panel-title" class="p-4 border-b border-gray-200 dark:border-gray-700 flex items-center justify-between">
        <h3 class="font-semibold text-gray-800 dark:text-gray-100">{$t('console.history')}</h3>
        <button class="text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 cursor-pointer text-lg" onclick={() => historyOpen = false} aria-label={$t('common.close')}>✕</button>
      </div>
      <div class="p-4">
        {#if history.length === 0}
          <p class="text-gray-400 dark:text-gray-500 text-sm text-center py-4">{$t('console.noResult')}</p>
        {:else}
          {#each history as item (item.id)}
            <div
              role="button"
              tabindex="0"
              class="mb-3 p-3 border border-gray-200 dark:border-gray-700 rounded hover:bg-gray-50 dark:hover:bg-gray-700/30 cursor-pointer"
              onclick={() => handleLoadHistory(item)}
              onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') handleLoadHistory(item); }}
              title={$t('console.historyLoadHint')}
            >
              <p class="text-xs font-mono text-gray-700 dark:text-gray-300 truncate mb-1">{item.query}</p>
              <div class="flex items-center gap-2 text-xs text-gray-400">
                <span class={item.success ? 'text-green-500' : 'text-red-500'}>{item.success ? '✓' : '✗'}</span>
                <span>{item.executionTime}ms</span>
                <span>{item.rowCount} {$t('console.rows')}</span>
                {#if item.path}
                  <span>· {$t(item.path === 'stream' ? 'console.historyViaStream' : 'console.historyViaMaterialized')}</span>
                {/if}
                {#if item.streamStatus}
                  <span>· {$t(item.streamStatus === 'completed' ? 'console.streamStatusCompleted' : item.streamStatus === 'cancelled' ? 'console.streamStatusCancelled' : 'console.streamStatusFailed')}</span>
                {/if}
                {#if item.path === 'stream' && item.reportedTotal !== undefined && item.reportedTotal !== null}
                  <span>· {$t('console.historyReceivedTotal', { values: { received: item.receivedCount ?? item.rowCount, total: item.reportedTotal } })}</span>
                {/if}
                {#if item.errorCode}
                  <span class="text-red-400">· {item.errorCode}</span>
                {/if}
                {#if item.traceId}
                  <span class="font-mono" title={item.traceId}>· ⛁ {item.traceId.slice(0, 8)}</span>
                {/if}
              </div>
              <button
                class="mt-1 text-xs text-blue-500 dark:text-blue-400 hover:text-blue-700 dark:hover:text-blue-300 cursor-pointer"
                title={$t('console.historyRerunHint')}
                onclick={(e) => { e.stopPropagation(); handleRerunHistory(item); }}
              >
                ↻ {$t('console.historyRerun')}
              </button>
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
    <div class="absolute inset-0 bg-black/20" role="presentation" onclick={() => favoritesOpen = false}></div>
    <div class="relative w-96 bg-white dark:bg-[#1C2333] shadow-lg h-full overflow-y-auto" role="dialog" aria-labelledby="favorites-panel-title">
      <div id="favorites-panel-title" class="p-4 border-b border-gray-200 dark:border-gray-700 flex items-center justify-between">
        <h3 class="font-semibold text-gray-800 dark:text-gray-100">{$t('console.favorites')}</h3>
        <button class="text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 cursor-pointer text-lg" onclick={() => favoritesOpen = false} aria-label={$t('common.close')}>✕</button>
      </div>
      <div class="p-4">
        {#if favorites.length === 0}
          <p class="text-gray-400 dark:text-gray-500 text-sm text-center py-4">{$t('console.noResult')}</p>
        {:else}
          {#each favorites as fav (fav.id)}
            <div
              role="button"
              tabindex="0"
              class="mb-3 p-3 border border-gray-200 dark:border-gray-700 rounded hover:bg-gray-50 dark:hover:bg-gray-700/30 cursor-pointer"
              onclick={() => handleLoadFavorite(fav)}
              onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') handleLoadFavorite(fav); }}
              title={$t('console.historyLoadHint')}
            >
              <p class="text-sm font-medium text-gray-800 dark:text-gray-200 mb-1">{fav.name}</p>
              <p class="text-xs font-mono text-gray-500 dark:text-gray-400 truncate">{fav.query}</p>
              {#if fav.preferredPath}
                <p class="text-xs text-gray-400 mt-1">{$t('console.favoriteSavedPath', { values: { path: $t(fav.preferredPath === 'stream' ? 'console.execStream' : fav.preferredPath === 'auto' ? 'console.execAuto' : 'console.execMaterialized') } })}</p>
              {/if}
              <div class="flex gap-3">
                <button
                  class="mt-1 text-xs text-blue-500 dark:text-blue-400 hover:text-blue-700 dark:hover:text-blue-300 cursor-pointer"
                  title={$t('console.historyRerunHint')}
                  onclick={(e) => { e.stopPropagation(); handleRerunFavorite(fav); }}
                >
                  ↻ {$t('console.historyRerun')}
                </button>
                <button
                  class="mt-1 text-xs text-red-400 hover:text-red-600 cursor-pointer"
                  onclick={(e) => { e.stopPropagation(); consoleStore.removeFromFavorites(fav.id); }}
                >
                  {$t('common.delete')}
                </button>
              </div>
            </div>
          {/each}
        {/if}
      </div>
    </div>
  </div>
{/if}

<!-- Save Favorite Modal -->
{#if saveModalOpen}
  <div class="fixed inset-0 z-50 flex items-center justify-center" role="dialog" aria-labelledby="save-modal-title">
    <div class="absolute inset-0 bg-black/20" role="presentation" onclick={() => saveModalOpen = false}></div>
    <div id="save-modal-title" class="relative bg-white dark:bg-[#1C2333] rounded-lg shadow-lg p-6 w-96">
      <h3>{$t('console.saveFavorite')}</h3>
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