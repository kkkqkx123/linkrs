<script lang="ts">
	import { t } from '$i18n';
	import { type ExecutionPreference } from '$utils/autoRoute';

	interface Props {
		isExecuting: boolean;
		isBusy: boolean;
		isValidating: boolean;
		isExplaining: boolean;
		editorContent: string;
		resultMode: string | null;
		executionPreference: ExecutionPreference;
		autoStreamThreshold: number;
		autoDecision: { path: string; estimatedRows: number | null; threshold: number } | null;
		streamEligibility: { eligible: boolean; mode: string | null; reason: string | null; count: number };
		parametersCount: number;
		historyCount: number;
		favoritesCount: number;
		onExecute: () => void;
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
	}

	let {
		isExecuting,
		isBusy,
		isValidating,
		isExplaining,
		editorContent,
		resultMode,
		executionPreference,
		autoStreamThreshold,
		autoDecision,
		streamEligibility,
		parametersCount,
		historyCount,
		favoritesCount,
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
	}: Props = $props();
</script>

<div class="px-4 pb-3 flex items-center gap-2 flex-wrap">
	<button
		class="px-4 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded transition-colors disabled:opacity-50 cursor-pointer"
		onclick={onExecute}
		disabled={isBusy || !editorContent.trim()}
	>
		{isExecuting ? t('console.executing') : t('console.execute')}
	</button>
	{#if isExecuting && resultMode === 'materialized'}
		<button
			class="px-4 py-1.5 bg-red-500 hover:bg-red-600 text-white text-sm rounded transition-colors cursor-pointer"
			onclick={onCancel}
		>
			{t('console.cancel')}
		</button>
	{/if}
	<button
		class="px-4 py-1.5 bg-teal-500 hover:bg-teal-600 text-white text-sm rounded transition-colors disabled:opacity-50 cursor-pointer"
		onclick={onStreamExecute}
		disabled={isBusy || !streamEligibility.eligible}
		title={streamEligibility.mode === 'single' && parametersCount > 0
			? t('console.streamParamsIgnored')
			: undefined}
	>
		{t('console.execStream')}
	</button>
	<button
		class="px-4 py-1.5 bg-indigo-500 hover:bg-indigo-600 text-white text-sm rounded transition-colors disabled:opacity-50 cursor-pointer"
		onclick={onCursorExecute}
		disabled={isBusy || streamEligibility.mode !== 'single'}
		title={t('console.cursorExecuteHint')}
	>
		{t('console.cursorExecute')}
	</button>
	{#if editorContent.trim() && !streamEligibility.eligible && streamEligibility.reason !== 'empty'}
		<span class="text-xs text-gray-400 dark:text-gray-500">
			{t('console.streamReasonCommand')}
		</span>
	{:else if streamEligibility.mode === 'batch'}
		<span class="text-xs text-gray-400 dark:text-gray-500">
			{t('console.streamBatchHint', { count: streamEligibility.count })}
		</span>
	{/if}
	<button
		class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded transition-colors cursor-pointer disabled:opacity-50"
		onclick={onValidate}
		disabled={isValidating || !editorContent.trim()}
	>
		{isValidating ? t('console.validating') : t('console.validate')}
	</button>
	<button
		class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded transition-colors cursor-pointer disabled:opacity-50"
		onclick={onExplain}
		disabled={isExplaining || isBusy || !editorContent.trim()}
		title={t('console.explainHint')}
	>
		{isExplaining ? t('console.explaining') : t('console.explain')}
	</button>
	<button
		class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded transition-colors cursor-pointer disabled:opacity-50"
		onclick={onFormat}
		disabled={!editorContent.trim()}
	>
		{t('console.format')}
	</button>
	<select
		class="px-2 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer"
		value={executionPreference}
		onchange={(e) => onPreferenceChange(e.currentTarget.value as ExecutionPreference)}
		title={t('console.execPreferenceHint')}
	>
		<option value="materialized">{t('console.execMaterialized')}</option>
		<option value="stream">{t('console.execStream')}</option>
		<option value="auto">{t('console.execAuto')}</option>
	</select>
	{#if executionPreference === 'auto'}
		<label class="flex items-center gap-1 text-xs text-gray-500 dark:text-gray-400">
			{t('console.autoThreshold')}
			<input
				type="number"
				min="1"
				max="10000000"
				step="100"
				class="w-24 px-2 py-1 border border-gray-300 dark:border-gray-600 rounded text-xs font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
				value={autoStreamThreshold}
				onchange={(e) => onThresholdChange(Number(e.currentTarget.value))}
			/>
		</label>
	{/if}
	<button
		class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded transition-colors cursor-pointer"
		onclick={onClear}
	>
		{t('common.clear')}
	</button>
	<div class="flex-1"></div>
	<button
		class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded transition-colors cursor-pointer"
		onclick={onToggleHistory}
	>
		📋 {t('console.history')} ({historyCount})
	</button>
	<button
		class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded transition-colors cursor-pointer"
		onclick={onToggleFavorites}
	>
		⭐ {t('console.favorites')} ({favoritesCount})
	</button>
	<button
		class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded transition-colors cursor-pointer"
		onclick={onSaveFavorite}
		disabled={!editorContent.trim()}
	>
		💾 {t('common.save')}
	</button>
</div>
{#if autoDecision}
	<div class="mx-4 mb-3 p-2 text-xs rounded border border-blue-200 dark:border-blue-800 bg-blue-50 dark:bg-blue-900/20 text-blue-700 dark:text-blue-300">
		{#if autoDecision.estimatedRows === null}
			{t('console.autoDecisionUnknown', { threshold: autoDecision.threshold })}
		{:else if autoDecision.path === 'stream'}
			{t('console.autoDecisionStream', {
				estimated: autoDecision.estimatedRows,
				threshold: autoDecision.threshold,
			})}
		{:else}
			{t('console.autoDecisionMaterialized', {
				estimated: autoDecision.estimatedRows,
				threshold: autoDecision.threshold,
			})}
		{/if}
	</div>
{/if}
