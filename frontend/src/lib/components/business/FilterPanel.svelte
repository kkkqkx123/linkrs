<script lang="ts">
	import { onMount } from 'svelte';
	import { message, t } from '$i18n';
	import { dataBrowserStore } from '$stores/dataBrowser';
	import { schemaStore } from '$stores/schema';
	import type { FilterOperator, FilterCondition } from '$types/dataBrowser';
	import type { Tag, EdgeType } from '$types/schema';

	let {
		activeTab,
		onApply,
	}: {
		activeTab: 'vertices' | 'edges';
		onApply: () => void;
	} = $props();

	const OPERATORS: { value: FilterOperator; label: () => string }[] = [
		{ value: 'eq', label: message('dataBrowser.op.eq') },
		{ value: 'ne', label: message('dataBrowser.op.ne') },
		{ value: 'gt', label: message('dataBrowser.op.gt') },
		{ value: 'lt', label: message('dataBrowser.op.lt') },
		{ value: 'ge', label: message('dataBrowser.op.ge') },
		{ value: 'le', label: message('dataBrowser.op.le') },
		{ value: 'contains', label: message('dataBrowser.op.contains') },
		{ value: 'startsWith', label: message('dataBrowser.op.startsWith') },
		{ value: 'endsWith', label: message('dataBrowser.op.endsWith') },
	];

	let conditions = $state<FilterCondition[]>([]);
	let logic = $state<'AND' | 'OR'>('AND');
	let availableProperties = $state<string[]>([]);
	let selectedTag = $state<string | null>(null);
	let selectedEdgeType = $state<string | null>(null);
	let tags = $state<Tag[]>([]);
	let edgeTypes = $state<EdgeType[]>([]);

	onMount(() => {
		const unsubBrowser = dataBrowserStore.subscribe((s) => {
			conditions = s.filters.conditions;
			logic = s.filters.logic;
			selectedTag = s.selectedTag;
			selectedEdgeType = s.selectedEdgeType;
		});
		const unsubSchema = schemaStore.subscribe((s) => {
			tags = s.tags;
			edgeTypes = s.edgeTypes;
		});
		return () => {
			unsubBrowser();
			unsubSchema();
		};
	});

	$effect(() => {
		if (activeTab === 'vertices') {
			const tag = tags.find((item) => item.name === selectedTag);
			availableProperties = (tag?.properties ?? []).map((p) => p.name);
		} else {
			const edge = edgeTypes.find((item) => item.name === selectedEdgeType);
			availableProperties = (edge?.properties ?? []).map((p) => p.name);
		}
	});

	function addCondition() {
		dataBrowserStore.addFilterCondition({
			property: availableProperties[0] ?? '',
			operator: 'eq',
			value: '',
		});
	}

	function removeCondition(index: number) {
		dataBrowserStore.removeFilterCondition(index);
	}

	function updateCondition(index: number, patch: Partial<FilterCondition>) {
		const next = conditions.map((condition, i) =>
			i === index ? { ...condition, ...patch } : condition,
		);
		dataBrowserStore.setFilters({ conditions: next, logic });
	}

	function handleLogicChange(nextLogic: 'AND' | 'OR') {
		dataBrowserStore.setFilters({ conditions, logic: nextLogic });
	}

	function handleApply() {
		onApply();
	}

	function handleClear() {
		dataBrowserStore.clearFilters();
		onApply();
	}
</script>

<div
	class="rounded-lg border border-gray-200 dark:border-gray-700 bg-white dark:bg-[#1C2333] p-4 flex flex-col gap-3"
>
	<div class="flex items-center justify-between">
		<h3 class="text-sm font-semibold text-gray-800 dark:text-gray-100">
			{t('dataBrowser.filterPanel.title')}
		</h3>
		<div class="flex items-center gap-1 text-xs">
			<button
				class="px-2 py-0.5 rounded cursor-pointer {logic === 'AND'
					? 'bg-blue-100 dark:bg-blue-900/30 text-blue-600 dark:text-blue-400'
					: 'text-gray-500 dark:text-gray-400'}"
				onclick={() => handleLogicChange('AND')}
				>{t('dataBrowser.filterPanel.logicAnd')}</button
			>
			<button
				class="px-2 py-0.5 rounded cursor-pointer {logic === 'OR'
					? 'bg-blue-100 dark:bg-blue-900/30 text-blue-600 dark:text-blue-400'
					: 'text-gray-500 dark:text-gray-400'}"
				onclick={() => handleLogicChange('OR')}
				>{t('dataBrowser.filterPanel.logicOr')}</button
			>
		</div>
	</div>

	{#if conditions.length === 0}
		<p class="text-xs text-gray-400 dark:text-gray-500">
			{t('dataBrowser.filterPanel.empty')}
		</p>
	{:else}
		<div class="flex flex-col gap-2">
			{#each conditions as condition, index (index)}
				<div class="flex items-center gap-2">
					<select
						class="flex-1 px-2 py-1.5 border border-gray-300 dark:border-gray-600 rounded text-xs bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
						value={condition.property}
						onchange={(e) =>
							updateCondition(index, {
								property: (e.target as HTMLSelectElement).value,
							})}
					>
						{#if availableProperties.length === 0}
							<option value="">{t('dataBrowser.filterPanel.property')}</option>
						{/if}
						{#each availableProperties as prop (prop)}
							<option value={prop}>{prop}</option>
						{/each}
					</select>
					<select
						class="px-2 py-1.5 border border-gray-300 dark:border-gray-600 rounded text-xs bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
						value={condition.operator}
						onchange={(e) =>
							updateCondition(index, {
								operator: (e.target as HTMLSelectElement)
									.value as FilterOperator,
							})}
					>
						{#each OPERATORS as op (op.value)}
							<option value={op.value}>{op.label()}</option>
						{/each}
					</select>
					<input
						type="text"
						class="flex-1 px-2 py-1.5 border border-gray-300 dark:border-gray-600 rounded text-xs bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
						placeholder={t('dataBrowser.filterPanel.value')}
						value={String(condition.value)}
						oninput={(e) =>
							updateCondition(index, {
								value: (e.target as HTMLInputElement).value,
							})}
					/>
					<button
						class="text-red-400 hover:text-red-600 cursor-pointer px-1"
						onclick={() => removeCondition(index)}>✕</button
					>
				</div>
			{/each}
		</div>
	{/if}

	<div class="flex items-center gap-2">
		<button
			class="text-blue-500 hover:text-blue-700 text-xs cursor-pointer"
			onclick={addCondition}
			>+ {t('dataBrowser.filterPanel.addCondition')}</button
		>
		<div class="flex-1"></div>
		<button
			class="px-3 py-1 border border-gray-300 dark:border-gray-600 rounded text-xs text-gray-700 dark:text-gray-300 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 cursor-pointer"
			onclick={handleClear}
		>
			{t('common.clear')}
		</button>
		<button
			class="px-3 py-1 bg-blue-500 hover:bg-blue-600 text-white text-xs rounded cursor-pointer"
			onclick={handleApply}
		>
			{t('dataBrowser.filterPanel.apply')}
		</button>
	</div>
</div>
