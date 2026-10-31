<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { t } from '$i18n';
	import { fromStore } from 'svelte/store';
	import {
		consoleStore,
		type QueryHistoryItem,
		type QueryFavoriteItem,
	} from '$stores/console';
	import { graphStore } from '$stores/graph';
	import { historyStore } from '$stores/history';
	import PageHeader from '$components/layout/PageHeader.svelte';
	import { streamStore } from '$stores/stream';
	import { cursorStore } from '$stores/cursor';
	import { queryResultToGraph } from '$utils/cytoscapeConfig';
	import EditorPanel from '$components/business/workbench/EditorPanel.svelte';
	import ResultPanel from '$components/business/workbench/ResultPanel.svelte';
	import HistorySidebar from '$components/business/HistorySidebar.svelte';
	import FavoritesSidebar from '$components/business/FavoritesSidebar.svelte';
	import SaveQueryModal from '$components/business/SaveQueryModal.svelte';
	import { installKeymap } from '$utils/keymap';
	import type { QueryResult } from '$types/query';

	const consoleState = fromStore(consoleStore);
	const historyState = fromStore(historyStore);
	const streamState = fromStore(streamStore);
	const cursorState = fromStore(cursorStore);
	const graphState = fromStore(graphStore);

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
	let history = $derived(historyState.current.history);
	let favorites = $derived(historyState.current.favorites);
	let historyOpen = $state(false);
	let favoritesOpen = $state(false);
	let saveModalOpen = $state(false);
	let favoriteName = $state('');
	let saveModalError = $state('');
	// Graph workbench replaces the result area; fed from graphStore so the
	// exploration context survives view switches. Other pages (e.g. data
	// browser) open it by pushing data into graphStore from anywhere.
	let workbenchOpen = $derived(graphState.current.workbenchOpen);
	let editorPanelRef = $state<{
		focusEditor: () => void;
		jumpToPosition: (line: number, column: number) => void;
	} | null>(null);

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

	/** Toolbar busy state covering materialized, streaming, and cursor runs. */
	let isBusy = $derived(isExecuting || streamingActive || cursorBusy);

	onMount(() => {
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
		};
	});

	onDestroy(() => {
		consoleStore.cancelStream();
		void consoleStore.closeCursor();
	});

	/**
	 * Execute the text handed over by the editor, which is either the current
	 * selection, the statement under the caret, or the whole buffer.
	 */
	function handleExecute(text?: string) {
		if (typeof text === 'string' && text.trim()) {
			consoleStore.executeQueryByText(text);
			return;
		}
		consoleStore.setEditorContent(editorContent);
		consoleStore.executeQuery();
	}

	/** Run the full buffer through the streaming endpoint when eligible. */
	function handleStreamExecute() {
		if (isBusy) return;
		consoleStore.setEditorContent(editorContent);
		void consoleStore.startStream(editorContent);
	}

	/** Page through a single statement with a server cursor. */
	function handleCursorExecute() {
		if (isBusy) return;
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

	/** Jump the editor caret to a server-reported error position. */
	function handleJumpToError(position: { line: number; column: number }) {
		editorPanelRef?.jumpToPosition(position.line, position.column);
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
</script>

<div class="ui-page animate-fade-in">
	<PageHeader title={t('console.title')} />

	<!-- Editor Section -->
	<EditorPanel
		bind:this={editorPanelRef}
		bind:editorContent
		onExecute={handleExecute}
		onCancel={() => consoleStore.cancelMaterialized()}
		onStreamExecute={handleStreamExecute}
		onCursorExecute={handleCursorExecute}
		onToggleHistory={() => (historyOpen = !historyOpen)}
		onToggleFavorites={() => (favoritesOpen = !favoritesOpen)}
		onSaveFavorite={() => {
			favoriteName = '';
			saveModalError = '';
			saveModalOpen = true;
		}}
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
