<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { t } from '$i18n';
	import { get } from 'svelte/store';
	import { goto } from '$app/navigation';
	import {
		consoleStore,
		type QueryHistoryItem,
		type QueryFavoriteItem,
		type StatementResultEntry,
		type StreamState,
		type CursorState,
		type ResultMode,
		type AutoDecision,
	} from '$stores/console';
	import {
		clampAutoThreshold,
		type ExecutionPreference,
	} from '$utils/autoRoute';
	import { graphStore } from '$stores/graph';
	import { historyStore } from '$stores/history';
	import { streamStore } from '$stores/stream';
	import { cursorStore } from '$stores/cursor';
	import { theme } from '$stores/theme';
	import { queryResultToGraph } from '$utils/cytoscapeConfig';
	import CypherEditor from '$components/common/CypherEditor.svelte';
	import StreamingResult from '$components/business/StreamingResult.svelte';
	import CursorResult from '$components/business/CursorResult.svelte';
	import QueryToolbar from '$components/business/QueryToolbar.svelte';
	import ParametersPanel from '$components/business/ParametersPanel.svelte';
	import MaterializedResult from '$components/business/MaterializedResult.svelte';
	import HistorySidebar from '$components/business/HistorySidebar.svelte';
	import FavoritesSidebar from '$components/business/FavoritesSidebar.svelte';
	import SaveQueryModal from '$components/business/SaveQueryModal.svelte';
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
	let historyCount = $derived(history.length);
	let favoritesCount = $derived(favorites.length);
	let historyOpen = $state(false);
	let favoritesOpen = $state(false);
	let saveModalOpen = $state(false);
	let favoriteName = $state('');
	let saveModalError = $state('');
	let validateMessage = $state('');
	let isValidating = $state(false);
	let isExplaining = $state(false);
	let planResult = $state<QueryResult | null>(null);
	let planError = $state<QueryError | null>(null);
	let planOpen = $state(false);
	let editorRef = $state<{ jumpToPosition: (line: number, column: number) => void } | null>(null);

	interface ParamRow {
		id: string;
		name: string;
		text: string;
	}

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
	let cursorStatement = $state('');

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

	onMount(() => {
		const snapshot = get(consoleStore);
		parameters = rowsFromRecord(snapshot.parameters);
		sessionVariables = rowsFromRecord(snapshot.sessionVariables);
		const unsub = consoleStore.subscribe((s) => {
			editorContent = s.editorContent;
			isExecuting = s.isExecuting;
			currentResult = s.currentResult;
			results = s.results;
			executionTime = s.executionTime;
			error = s.error;
			activeView = s.activeView;
			resultMode = s.resultMode;
			executionPreference = s.executionPreference;
			autoStreamThreshold = s.autoStreamThreshold;
			autoDecision = s.autoDecision;
		});
		const unsubHistory = historyStore.subscribe((s) => {
			history = s.history;
			favorites = s.favorites;
		});
		const unsubStream = streamStore.subscribe((s) => {
			stream = s;
		});
		const unsubCursor = cursorStore.subscribe((s) => {
			cursor = s;
		});
		const unsubTheme = theme.subscribe((v) => {
			isDark = v === 'dark';
		});
		return () => {
			unsub();
			unsubHistory();
			unsubStream();
			unsubCursor();
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

	// Re-check the statement under the caret after the user pauses, so mistakes
	// surface without validating the whole script on every keystroke. Manual
	// validation stays available for an immediate full-script answer.
	$effect(() => {
		const statement = cursorStatement;
		if (validateTimer) clearTimeout(validateTimer);
		if (!statement.trim()) {
			validateMessage = '';
			return;
		}
		validateTimer = setTimeout(() => void autoValidate(statement), 800);
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
		goto('/graph');
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
		goto('/graph');
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

	/** Plan the statement under the cursor (or full buffer) without executing it. */
	async function handleExplain() {
		const target = cursorStatement.trim() ? cursorStatement : editorContent;
		if (!target.trim() || isExplaining || isBusy) return;
		isExplaining = true;
		planResult = null;
		planError = null;
		try {
			const outcome = await queryService.explain({ query: target });
			if (outcome.success && outcome.data) {
				planResult = outcome.data;
				planOpen = true;
			} else {
				planError = outcome.error ?? {
					code: 'EXECUTION_ERROR',
					message: t('errors.executeQuery'),
				};
				planOpen = true;
			}
		} finally {
			isExplaining = false;
		}
	}

	/** Jump the editor caret to a server-reported error position. */
	function handleJumpToError(position: { line: number; column: number }) {
		editorRef?.jumpToPosition(position.line, position.column);
	}

	function handleFormat() {
		const formatted = formatQuery(editorContent);
		if (formatted && formatted !== editorContent) {
			editorContent = formatted;
			consoleStore.setEditorContent(formatted);
		}
	}

	let validateTimer: ReturnType<typeof setTimeout> | null = null;

	/** Quietly re-check the statement under the caret after the user pauses typing. */
	async function autoValidate(statement: string) {
		if (isValidating) return;
		const stmt = statement.trim();
		if (!stmt) return;
		try {
			const outcome = await queryService.validate(stmt);
			if (stmt !== cursorStatement.trim()) return;
			validateMessage = outcome.message;
		} catch {
			/* ignore background check failures */
		}
	}

	function handleSaveFavorite() {
		if (!favoriteName.trim()) {
			saveModalError = t('errors.favoriteNameRequired');
			return;
		}
		const result = consoleStore.addToFavorites(favoriteName, editorContent);
		if (result.success) {
			saveModalOpen = false;
			favoriteName = '';
			saveModalError = '';
		} else {
			saveModalError = result.error || t('notification.saveFailed');
		}
	}

	function handleOpenInGraph() {
		if (!currentResult) return;
		const parsed = queryResultToGraph(currentResult);
		graphStore.setGraphData({ nodes: parsed.nodes, edges: parsed.edges });
		goto('/graph');
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

	function handleMaterializedViewChange(view: 'table' | 'json' | 'graph') {
		activeView = view;
		consoleStore.setActiveView(view);
	}
</script>

<div class="flex flex-col h-full gap-4 animate-fade-in">
	<!-- Header -->
	<div class="bg-white dark:bg-[#1C2333] rounded-lg shadow-sm px-5 py-3">
		<h2
			class="text-lg font-semibold text-gray-800 dark:text-gray-100 flex items-center gap-2"
		>
			<span>⌨</span>
			{t('console.title')}
		</h2>
	</div>

	<!-- Editor Section -->
	<div class="bg-white dark:bg-[#1C2333] rounded-lg shadow-sm flex flex-col">
		<div class="p-4 pb-2">
			<CypherEditor
				bind:this={editorRef}
				bind:value={editorContent}
				{isDark}
				placeholder="{t('console.queryPlaceholder')} {t('console.executeHint')}"
				onExecute={handleExecute}
				onCursorStatement={(text) => (cursorStatement = text)}
				historyProvider={() => history.map((item) => item.query)}
			/>
		</div>
		<QueryToolbar
			{isExecuting}
			{isBusy}
			{isValidating}
			{isExplaining}
			{editorContent}
			{resultMode}
			{executionPreference}
			{autoStreamThreshold}
			{autoDecision}
			{streamEligibility}
			parametersCount={parameters.length}
			{historyCount}
			{favoritesCount}
			onExecute={handleExecute}
			onCancel={() => consoleStore.cancelMaterialized()}
			onStreamExecute={handleStreamExecute}
			onCursorExecute={handleCursorExecute}
			onValidate={handleValidate}
			onExplain={handleExplain}
			onFormat={handleFormat}
			onClear={() => {
				consoleStore.setEditorContent('');
				consoleStore.clearResult();
			}}
			onToggleHistory={() => (historyOpen = !historyOpen)}
			onToggleFavorites={() => (favoritesOpen = !favoritesOpen)}
			onSaveFavorite={() => {
				favoriteName = '';
				saveModalError = '';
				saveModalOpen = true;
			}}
			onPreferenceChange={(pref) => consoleStore.setExecutionPreference(pref)}
			onThresholdChange={(val) =>
				consoleStore.setAutoStreamThreshold(clampAutoThreshold(val))}
		/>
			<button
				class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded transition-colors cursor-pointer"
				onclick={() => (historyOpen = !historyOpen)}
			>
				📋 {t('console.history')} ({history.length})
			</button>
			<button
				class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded transition-colors cursor-pointer"
				onclick={() => (favoritesOpen = !favoritesOpen)}
			>
				⭐ {t('console.favorites')} ({favorites.length})
			</button>
		</div>
		<ParametersPanel
			{parameters}
			{sessionVariables}
			open={paramsOpen}
			{streamEligibility}
			{streamingActive}
			onToggle={() => (paramsOpen = !paramsOpen)}
			onParametersChange={(rows) => (parameters = rows)}
			onSessionVariablesChange={(rows) => (sessionVariables = rows)}
		/>
		{#if validateMessage}
			<div
				class="mx-4 mb-3 p-2 text-xs rounded border border-gray-200 dark:border-gray-700 bg-gray-50 dark:bg-gray-800/50 text-gray-600 dark:text-gray-300"
			>
				{validateMessage}
			</div>
		{/if}
		{#if planOpen}
			<div class="mx-4 mb-3 rounded border border-purple-200 dark:border-purple-800 bg-purple-50 dark:bg-purple-900/20 overflow-hidden">
				<div class="px-3 py-2 flex items-center gap-2 text-sm text-purple-700 dark:text-purple-300">
					<span class="font-medium">{t('console.planTitle')}</span>
					<div class="flex-1"></div>
					<button
						class="text-xs px-2 py-0.5 border border-purple-300 dark:border-purple-700 rounded hover:bg-purple-100 dark:hover:bg-purple-900/40 cursor-pointer"
						onclick={() => (planOpen = false)}
					>
						{t('common.close')}
					</button>
				</div>
				<div class="px-3 pb-3">
					{#if planError}
						<div class="text-red-600 dark:text-red-400 text-xs">
							<span class="font-medium">{planError.code}</span>: {planError.message}
							{#if planError.position}
								<button
									class="ml-2 px-2 py-0.5 text-xs border border-red-300 dark:border-red-700 rounded hover:bg-red-100 dark:hover:bg-red-900/40 cursor-pointer"
									onclick={() => handleJumpToError(planError!.position!)}
								>
									{t('console.jumpToError', {
										line: planError!.position!.line,
										column: planError!.position!.column,
									})}
								</button>
							{/if}
						</div>
					{:else if planResult}
						{#if planResult.columns.length > 0}
							<div class="overflow-x-auto bg-white dark:bg-[#1C2333] rounded border border-purple-200 dark:border-purple-800">
								<table class="w-full text-xs border-collapse">
									<thead>
										<tr class="bg-purple-100/50 dark:bg-purple-900/30">
											{#each planResult.columns as col (col)}
												<th class="px-3 py-1.5 text-left font-medium whitespace-nowrap">{col}</th>
											{/each}
										</tr>
									</thead>
									<tbody>
										{#each planResult.rows as row, i (i)}
											<tr class="even:bg-purple-50/50 dark:even:bg-purple-900/10">
												{#each planResult.columns as col (col)}
													<td class="px-3 py-1 border-t border-purple-100 dark:border-purple-800/50 font-mono max-w-md truncate">{String(row[col] ?? '')}</td>
												{/each}
											</tr>
										{/each}
									</tbody>
								</table>
							</div>
						{:else}
							<div class="text-xs text-green-600 dark:text-green-400">{t('common.ok')}</div>
						{/if}
					{/if}
				</div>
			</div>
		{/if}
	</div>

	<!-- Result Section -->
	{#if resultMode === 'stream' && stream}
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
	{:else}
		<MaterializedResult
			{isExecuting}
			{currentResult}
			{results}
			{executionTime}
			{error}
			{activeView}
			{isDark}
			onViewChange={handleMaterializedViewChange}
			onOpenInGraph={handleOpenInGraph}
			onJumpToError={handleJumpToError}
		/>
	{/if}

<!-- History Panel -->
{#if historyOpen}
	<HistorySidebar
		{history}
		onClose={() => (historyOpen = false)}
		onLoad={handleLoadHistory}
		onRerun={handleRerunHistory}
		onClear={() => consoleStore.clearHistory()}
	/>
{/if}

<!-- Favorites Panel -->
{#if favoritesOpen}
	<FavoritesSidebar
		{favorites}
		onClose={() => (favoritesOpen = false)}
		onLoad={handleLoadFavorite}
		onRerun={handleRerunFavorite}
		onDelete={(id) => consoleStore.removeFromFavorites(id)}
	/>
{/if}

<!-- Save Favorite Modal -->
<SaveQueryModal
	open={saveModalOpen}
	name={favoriteName}
	error={saveModalError}
	onClose={() => (saveModalOpen = false)}
	onSave={handleSaveFavorite}
	onNameChange={(name) => (favoriteName = name)}
/>
