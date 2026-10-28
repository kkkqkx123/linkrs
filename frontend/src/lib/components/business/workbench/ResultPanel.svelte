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
		onStreamViewChange: (view: 'table' | 'json' | 'graph') => void;
		onCancelStream: () => void;
		onStreamOpenInGraph: (result: QueryResult) => void;
		onCursorViewChange: (view: 'table' | 'json' | 'graph') => void;
		onCursorLoadMore: () => void;
		onCursorClose: () => void;
		onCursorOpenInGraph: (result: QueryResult) => void;
		onMaterializedViewChange: (view: 'table' | 'json' | 'graph') => void;
		onOpenInGraph: () => void;
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
		onStreamViewChange,
		onCancelStream,
		onStreamOpenInGraph,
		onCursorViewChange,
		onCursorLoadMore,
		onCursorClose,
		onCursorOpenInGraph,
		onMaterializedViewChange,
		onOpenInGraph,
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
		onViewChange={onStreamViewChange}
		onCancel={onCancelStream}
		onOpenInGraph={onStreamOpenInGraph}
	/>
{:else if resultMode === 'cursor' && cursor}
	<CursorResult
		{cursor}
		{activeView}
		{isDark}
		onViewChange={onCursorViewChange}
		onLoadMore={onCursorLoadMore}
		onClose={onCursorClose}
		onOpenInGraph={onCursorOpenInGraph}
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
		onViewChange={onMaterializedViewChange}
		onOpenInGraph={onOpenInGraph}
		onJumpToError={onJumpToError}
	/>
{/if}
