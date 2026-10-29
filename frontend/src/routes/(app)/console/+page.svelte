<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { t } from '$i18n';
	import { get, fromStore } from 'svelte/store';
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
	import { graphStore } from '$stores/graph';
	import { historyStore } from '$stores/history';
	import PageHeader from '$components/layout/PageHeader.svelte';
	import { streamStore } from '$stores/stream';
	import { cursorStore } from '$stores/cursor';
	import { theme } from '$stores/theme';
	import { queryResultToGraph } from '$utils/cytoscapeConfig';
	import EditorPanel from '$components/business/workbench/EditorPanel.svelte';
	import ResultPanel from '$components/business/workbench/ResultPanel.svelte';
	import HistorySidebar from '$components/business/HistorySidebar.svelte';
	import FavoritesSidebar from '$components/business/FavoritesSidebar.svelte';
	import SaveQueryModal from '$components/business/SaveQueryModal.svelte';
	import { queryService } from '$services/query';
	import { formatQuery, getStreamEligibility, splitQueries } from '$utils/gql';
	import { installKeymap } from '$utils/keymap';
	import type { QueryResult, QueryError } from '$types/query';
	import type { ParamRow } from '$components/business/workbench/EditorPanel.svelte';

	const consoleState = fromStore(consoleStore);
	const historyState = fromStore(historyStore);
	const streamState = fromStore(streamStore);
	const cursorState = fromStore(cursorStore);
	const graphState = fromStore(graphStore);
	const themeState = fromStore(theme);

	let editorContent = $state('');
	let isExecuting = $derived(consoleState.current.isExecuting);
	let currentResult = $derived(consoleState.current.currentResult);
	let results = $derived(consoleState.current.results);
	let executionTime = $derived(consoleState.current.executionTime);
	let error = $derived(consoleState.current.error);
	let activeView = $derived(consoleState.current.activeView);
	let stream = $derived(streamState.current);
	let cursor = $derived(cursorState.current);
	let resultMode = $derived(consoleState.current.resultMode);
	let executionPreference = $derived(consoleState.current.executionPreference);
	let autoStreamThreshold = $derived(consoleState.current.autoStreamThreshold);
	let autoDecision = $derived(consoleState.current.autoDecision);
	let isDark = $derived(themeState.current === 'dark');
	let history = $derived(historyState.current.history);
	let favorites = $derived(historyState.current.favorites);
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
	// Graph workbench replaces the result area; fed from graphStore so the
	// exploration context survives view switches. Other pages (e.g. data
	// browser) open it by pushing data into graphStore from anywhere.
	let workbenchOpen = $derived(graphState.current.workbenchOpen);
	let editorPanelRef = $state<{
		focusEditor: () => void;
		jumpToPosition: (line: number, column: number) => void;
	} | null>(null);

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
		const uninstallKeymap = installKeymap({
			execute: () => handleExecute(),
			focusEditor: () => editorPanelRef?.focusEditor(),
			openHistory: () => (historyOpen = true),
			setView: (view) => consoleStore.setActiveView(view),
			isEditorFocused: () =>
				Boolean(
					document.activeElement &&
						document.activeElement.closest('.monaco-editor'),
				),
			isBusy: () => isBusy,
		});
		return () => {
			uninstallKeymap();
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

	function parseParamValue(text: string): unknown {
		const trimmed = text.trim();
		if (!trimmed) return '';
		try {
			return JSON.parse(trimmed);
		} catch {
			return text;
		}
	}

	let rowSeq = 0;
	const nextRowId = () => `param-${Date.now()}-${rowSeq++}`;

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

	function handleCancelStream() {
		consoleStore.cancelStream();
	}

	// One view-change handler for all transports: the active view is owned by
	// the console store, so the derived value updates through it.
	function handleViewChange(view: 'table' | 'json' | 'graph') {
		consoleStore.setActiveView(view);
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
		editorPanelRef?.jumpToPosition(position.line, position.column);
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

	function handleOpenInGraph(result: QueryResult) {
		const parsed = queryResultToGraph(result);
		graphStore.setGraphData({ nodes: parsed.nodes, edges: parsed.edges });
		graphStore.openWorkbench();
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

<div class="ui-page animate-fade-in">
	<PageHeader title={t('console.title')} />

	<!-- Editor Section -->
	<EditorPanel
		bind:this={editorPanelRef}
		bind:editorContent
		{isDark}
		{isValidating}
		{isExplaining}
		{parameters}
		{sessionVariables}
		{paramsOpen}
		{history}
		{favoritesCount}
		{validateMessage}
		{planOpen}
		{planResult}
		{planError}
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
		onToggleParams={() => (paramsOpen = !paramsOpen)}
		onParametersChange={(rows) => (parameters = rows)}
		onSessionVariablesChange={(rows) => (sessionVariables = rows)}
		onCursorStatement={(text) => (cursorStatement = text)}
		onClosePlan={() => (planOpen = false)}
		onJumpToError={handleJumpToError}
	/>

	<ResultPanel
		{workbenchOpen}
		{resultMode}
		{stream}
		{cursor}
		{isExecuting}
		{currentResult}
		{results}
		{executionTime}
		{error}
		{activeView}
		{isDark}
		onCloseWorkbench={() => graphStore.closeWorkbench()}
		onViewChange={handleViewChange}
		onCancelStream={handleCancelStream}
		onOpenInGraph={handleOpenInGraph}
		onCursorLoadMore={handleCursorLoadMore}
		onCursorClose={handleCursorClose}
		onJumpToError={handleJumpToError}
	/>

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
</div>
