<script lang="ts">
	import { t } from '$i18n';

	interface ParamRow {
		id: string;
		name: string;
		text: string;
	}

	interface Props {
		parameters: ParamRow[];
		sessionVariables: ParamRow[];
		open: boolean;
		streamEligibility: { mode: string | null };
		streamingActive: boolean;
		onToggle: () => void;
		onParametersChange: (rows: ParamRow[]) => void;
		onSessionVariablesChange: (rows: ParamRow[]) => void;
	}

	let {
		parameters,
		sessionVariables,
		open,
		streamEligibility,
		streamingActive,
		onToggle,
		onParametersChange,
		onSessionVariablesChange,
	}: Props = $props();

	function addRow() {
		parameters = [...parameters, { id: `param-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`, name: '', text: '' }];
		onParametersChange(parameters);
	}

	function removeRow(id: string) {
		parameters = parameters.filter((r) => r.id !== id);
		onParametersChange(parameters);
	}

	function addSessionVariable() {
		sessionVariables = [...sessionVariables, { id: `sv-${Date.now()}-${Math.random().toString(36).substr(2, 9)}`, name: '', text: '' }];
		onSessionVariablesChange(sessionVariables);
	}

	function removeSessionVariable(id: string) {
		sessionVariables = sessionVariables.filter((r) => r.id !== id);
		onSessionVariablesChange(sessionVariables);
	}
</script>

<div class="px-4 pb-3">
	<button
		class="text-xs text-gray-500 dark:text-gray-400 hover:text-gray-700 dark:hover:text-gray-200 cursor-pointer"
		onclick={onToggle}
	>
		{open ? '▾' : '▸'}
		{t('console.parameters')} ({parameters.length + sessionVariables.length})
	</button>
	{#if open}
		<div class="mt-2 grid grid-cols-1 md:grid-cols-2 gap-3">
			<div class="border border-gray-200 dark:border-gray-700 rounded p-2">
				<p class="text-xs font-medium text-gray-600 dark:text-gray-300 mb-1">
					@ {t('console.parameters')}
				</p>
				{#each parameters as row (row.id)}
					<div class="flex gap-1 mb-1">
						<input
							type="text"
							bind:value={row.name}
							placeholder={t('common.name')}
							class="w-1/3 ui-input ui-input-sm font-mono"
						/>
						<input
							type="text"
							bind:value={row.text}
							placeholder={t('console.bindingValue')}
							class="flex-1 ui-input ui-input-sm font-mono"
						/>
						<button
							class="px-1.5 ui-link-danger text-xs"
							onclick={() => removeRow(row.id)}
							aria-label={t('common.delete')}>✕</button
						>
					</div>
				{/each}
				<button
					class="ui-link text-xs"
					onclick={addRow}
				>
					{t('console.addBinding')}
				</button>
			</div>
			<div class="border border-gray-200 dark:border-gray-700 rounded p-2">
				<p class="text-xs font-medium text-gray-600 dark:text-gray-300 mb-1">
					$ {t('console.sessionVariables')}
				</p>
				{#each sessionVariables as row (row.id)}
					<div class="flex gap-1 mb-1">
						<input
							type="text"
							bind:value={row.name}
							placeholder={t('common.name')}
							class="w-1/3 ui-input ui-input-sm font-mono"
						/>
						<input
							type="text"
							bind:value={row.text}
							placeholder={t('console.bindingValue')}
							class="flex-1 ui-input ui-input-sm font-mono"
						/>
						<button
							class="px-1.5 ui-link-danger text-xs"
							onclick={() => removeSessionVariable(row.id)}
							aria-label={t('common.delete')}>✕</button
						>
					</div>
				{/each}
				<button
					class="ui-link text-xs"
					onclick={addSessionVariable}
				>
					{t('console.addBinding')}
				</button>
			</div>
		</div>
		{#if (streamEligibility.mode === 'single' && !streamingActive) || streamingActive}
			<p class="mt-2 text-xs text-amber-600 dark:text-amber-400">
				{t('console.streamParamsIgnored')}
			</p>
		{/if}
	{/if}
</div>
