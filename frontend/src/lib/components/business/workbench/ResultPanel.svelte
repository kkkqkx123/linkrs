<script lang="ts">
	import { fromStore } from 'svelte/store';
	import { t } from '$i18n';
	import StreamingResult from '$components/business/StreamingResult.svelte';
	import CursorResult from '$components/business/CursorResult.svelte';
	import MaterializedResult from '$components/business/MaterializedResult.svelte';
	import GraphWorkbench from '$components/business/workbench/GraphWorkbench.svelte';
	import { theme } from '$stores/theme';
	import type {
		StatementResultEntry,
		StreamState,
		CursorState,
		ResultMode,
	} from '$stores/console';
	import type { QueryResult, QueryError } from '$types/query';

	interface Props {
		workbenchOpen: boolean;
		resultMode: ResultMode;
		stream: StreamState | null;
		cursor: CursorState | null;
		isExecuting: boolean;
		currentResult: QueryResult | null;
		results: StatementResultEntry[];
		executionTime: number;
		error: QueryError | null;
		activeView: 'table' | 'json' | 'graph';

		onCloseWorkbench: () => void;
		// Single view-change / open-in-graph pair for all transports; the
		// transport-specific duplication used to live in the page.
		onViewChange: (view: 'table' | 'json' | 'graph') => void;
		onCancelStream: () => void;
		onOpenInGraph: (result: QueryResult) => void;
		onCursorLoadMore: () => void;
		onCursorClose: () => void;
		onJumpToError: (position: { line: number; column: number }) => void;
	}

	let {
		workbenchOpen,
		resultMode,
		stream,
		cursor,
		isExecuting,
		currentResult,
		results,
		executionTime,
		error,
		activeView,
		onCloseWorkbench,
		onViewChange,
		onCancelStream,
		onOpenInGraph,
		onCursorLoadMore,
		onCursorClose,
		onJumpToError,
	}: Props = $props();

	// Theme is read from the store directly instead of being threaded through
	// the page as a prop.
	const themeState = fromStore(theme);
	let isDark = $derived(themeState.current === 'dark');
</script>

<!-- Result Section -->
{#if workbenchOpen}
	<div class="flex items-center justify-between mb-2">
		<span class="text-sm font-medium text-ink"
			>{t('graph.title')}</span
		>
		<button
			class="ui-btn ui-btn-ghost ui-btn-sm"
			onclick={onCloseWorkbench}
		>
			{t('common.close')}
		</button>
	</div>
	<GraphWorkbench />
{:else if resultMode === 'stream' && stream}
	<StreamingResult
		{stream}
		{activeView}
		{isDark}
		{onViewChange}
		onCancel={onCancelStream}
		{onOpenInGraph}
	/>
{:else if resultMode === 'cursor' && cursor}
	<CursorResult
		{cursor}
		{activeView}
		{isDark}
		{onViewChange}
		onLoadMore={onCursorLoadMore}
		onClose={onCursorClose}
		{onOpenInGraph}
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
		{onViewChange}
		{onOpenInGraph}
		onJumpToError={onJumpToError}
	/>
{/if}
