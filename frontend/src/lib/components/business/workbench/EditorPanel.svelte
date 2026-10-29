<script lang="ts">
	import { t } from '$i18n';
	import { History, Star } from '@lucide/svelte';
	import CypherEditor from '$components/common/CypherEditor.svelte';
	import QueryToolbar from '$components/business/QueryToolbar.svelte';
	import ParametersPanel from '$components/business/ParametersPanel.svelte';
	import type {
		ResultMode,
		AutoDecision,
	} from '$stores/console';
	import type { ExecutionPreference } from '$utils/autoRoute';
	import type { StreamEligibility } from '$utils/gql';
	import type { QueryResult, QueryError } from '$types/query';

	/** One editable binding row for query parameters or session variables. */
	export interface ParamRow {
		id: string;
		name: string;
		text: string;
	}

	interface Props {
		editorContent: string;
		isDark: boolean;
		isExecuting: boolean;
		isBusy: boolean;
		isValidating: boolean;
		isExplaining: boolean;
		resultMode: ResultMode;
		executionPreference: ExecutionPreference;
		autoStreamThreshold: number;
		autoDecision: AutoDecision | null;
		streamEligibility: StreamEligibility;
		streamingActive: boolean;
		parameters: ParamRow[];
		sessionVariables: ParamRow[];
		paramsOpen: boolean;
		history: { query: string }[];
		favoritesCount: number;
		validateMessage: string;
		planOpen: boolean;
		planResult: QueryResult | null;
		planError: QueryError | null;

		onExecute: (text?: string) => void;
		onCancel: () => void;
		onStreamExecute: () => void;
		onCursorExecute: () => void;
		onValidate: () => void;
		onExplain: () => void;
		onFormat: () => void;
		onClear: () => void;
		onToggleHistory: () => void;
		onToggleFavorites: () => void;
		onSaveFavorite: () => void;
		onPreferenceChange: (pref: ExecutionPreference) => void;
		onThresholdChange: (val: number) => void;
		onToggleParams: () => void;
		onParametersChange: (rows: ParamRow[]) => void;
		onSessionVariablesChange: (rows: ParamRow[]) => void;
		onCursorStatement: (text: string) => void;
		onClosePlan: () => void;
		onJumpToError: (position: { line: number; column: number }) => void;
	}

	let {
		editorContent = $bindable(''),
		isDark,
		isExecuting,
		isBusy,
		isValidating,
		isExplaining,
		resultMode,
		executionPreference,
		autoStreamThreshold,
		autoDecision,
		streamEligibility,
		streamingActive,
		parameters,
		sessionVariables,
		paramsOpen,
		history,
		favoritesCount,
		validateMessage,
		planOpen,
		planResult,
		planError,
		onExecute,
		onCancel,
		onStreamExecute,
		onCursorExecute,
		onValidate,
		onExplain,
		onFormat,
		onClear,
		onToggleHistory,
		onToggleFavorites,
		onSaveFavorite,
		onPreferenceChange,
		onThresholdChange,
		onToggleParams,
		onParametersChange,
		onSessionVariablesChange,
		onCursorStatement,
		onClosePlan,
		onJumpToError,
	}: Props = $props();

	let editorRef = $state<{
		focus: () => void;
		jumpToPosition: (line: number, column: number) => void;
	} | null>(null);

	/** Focus the underlying Cypher editor on behalf of the page keymap. */
	export function focusEditor() {
		editorRef?.focus();
	}

	/** Move the underlying editor caret to a reported error position. */
	export function jumpToPosition(line: number, column: number) {
		editorRef?.jumpToPosition(line, column);
	}
</script>

<div class="ui-card ui-card-flush shadow-sm flex flex-col">
	<div class="p-5 pb-3">
		<CypherEditor
			bind:this={editorRef}
			bind:value={editorContent}
			{isDark}
			placeholder="{t('console.queryPlaceholder')} {t('console.executeHint')}"
			onExecute={onExecute}
			onCursorStatement={onCursorStatement}
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
		historyCount={history.length}
		{favoritesCount}
		onExecute={onExecute}
		onCancel={onCancel}
		onStreamExecute={onStreamExecute}
		onCursorExecute={onCursorExecute}
		onValidate={onValidate}
		onExplain={onExplain}
		onFormat={onFormat}
		onClear={onClear}
		onToggleHistory={onToggleHistory}
		onToggleFavorites={onToggleFavorites}
		onSaveFavorite={onSaveFavorite}
		onPreferenceChange={onPreferenceChange}
		onThresholdChange={onThresholdChange}
	/>
	<button
		class="ui-btn ui-btn-ghost"
		onclick={onToggleHistory}
	>
		<History size={16} /> {t('console.history')} ({history.length})
	</button>
	<button
		class="ui-btn ui-btn-ghost"
		onclick={onToggleFavorites}
	>
		<Star size={16} /> {t('console.favorites')} ({favoritesCount})
	</button>
</div>
<ParametersPanel
	{parameters}
	{sessionVariables}
	open={paramsOpen}
	{streamEligibility}
	{streamingActive}
	onToggle={onToggleParams}
	onParametersChange={onParametersChange}
	onSessionVariablesChange={onSessionVariablesChange}
/>
{#if validateMessage}
	<div class="ui-alert ui-alert-neutral mx-4 mb-3">
		{validateMessage}
	</div>
{/if}
{#if planOpen}
	<div
		class="mx-4 mb-3 rounded border border-alt/40 bg-alt-light overflow-hidden"
	>
		<div
			class="px-3 py-2 flex items-center gap-2 text-sm text-alt"
		>
			<span class="font-medium">{t('console.planTitle')}</span>
			<div class="flex-1"></div>
			<button
				class="ui-btn ui-btn-ghost ui-btn-xs"
				onclick={onClosePlan}
			>
				{t('common.close')}
			</button>
		</div>
		<div class="px-3 pb-3">
			{#if planError}
				<div class="text-error text-xs">
					<span class="font-medium">{planError.code}</span>: {planError.message}
					{#if planError.position}
						<button
							class="ui-btn ui-btn-danger-outline ui-btn-sm"
							onclick={() => onJumpToError(planError!.position!)}
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
					<div
						class="overflow-x-auto bg-surface rounded border border-alt/40"
					>
						<table class="ui-table w-full border-collapse">
							<thead>
								<tr class="bg-alt-light/50">
									{#each planResult.columns as col (col)}
										<th>{col}</th>
									{/each}
								</tr>
							</thead>
							<tbody>
								{#each planResult.rows as row, i (i)}
									<tr class="even:bg-alt-light/50">
										{#each planResult.columns as col (col)}
											<td class="font-mono max-w-md truncate"
												>{String(row[col] ?? '')}</td
											>
										{/each}
									</tr>
								{/each}
							</tbody>
						</table>
					</div>
				{:else}
					<div class="text-xs text-success">
						{t('common.ok')}
					</div>
				{/if}
			{/if}
		</div>
	</div>
{/if}
