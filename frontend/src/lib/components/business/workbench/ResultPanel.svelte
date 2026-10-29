<script lang="ts">
	import { t } from '$i18n';
	import StreamingResult from '$components/business/StreamingResult.svelte';
	import CursorResult from '$components/business/CursorResult.svelte';
	import MaterializedResult from '$components/business/MaterializedResult.svelte';
	import GraphWorkbench from '$components/business/workbench/GraphWorkbench.svelte';
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
		isDark: boolean;

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
		isDark,
		onCloseWorkbench,
		onViewChange,
		onCancelStream,
		onOpenInGraph,
		onCursorLoadMore,
		onCursorClose,
		onJumpToError,
	}: Props = $props();
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
