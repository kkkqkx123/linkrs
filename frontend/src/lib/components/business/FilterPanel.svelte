<script lang="ts">
	import { onMount } from 'svelte';
	import { message, t } from '$i18n';
	import { dataBrowserStore } from '$stores/dataBrowser';
	import { schemaStore } from '$stores/schema';
	import type {
		FilterOperator,
		FilterCondition,
		DataBrowserTab,
	} from '$types/dataBrowser';
	import type { Tag, EdgeType } from '$types/schema';

	let {
		activeTab,
		onApply,
	}: {
		activeTab: DataBrowserTab;
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
	class="ui-card p-4 flex flex-col gap-3"
>
	<div class="flex items-center justify-between">
		<h3 class="text-sm font-semibold text-ink">
			{t('dataBrowser.filterPanel.title')}
		</h3>
		<div class="flex items-center gap-1 text-xs">
			<button
				class="ui-seg {logic === 'AND' ? 'ui-seg-active' : ''}"
				onclick={() => handleLogicChange('AND')}
				>{t('dataBrowser.filterPanel.logicAnd')}</button
			>
			<button
				class="ui-seg {logic === 'OR' ? 'ui-seg-active' : ''}"
				onclick={() => handleLogicChange('OR')}
				>{t('dataBrowser.filterPanel.logicOr')}</button
			>
		</div>
	</div>

	{#if conditions.length === 0}
		<p class="text-xs text-muted">
			{t('dataBrowser.filterPanel.empty')}
		</p>
	{:else}
		<div class="flex flex-col gap-2">
			{#each conditions as condition, index (index)}
				<div class="flex items-center gap-2">
					<select
						class="flex-1 ui-input ui-input-sm"
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
						class="ui-input ui-input-sm"
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
						class="flex-1 ui-input ui-input-sm"
						placeholder={t('dataBrowser.filterPanel.value')}
						value={String(condition.value)}
						oninput={(e) =>
							updateCondition(index, {
								value: (e.target as HTMLInputElement).value,
							})}
					/>
					<button
						class="ui-link-danger px-1"
						onclick={() => removeCondition(index)}>✕</button
					>
				</div>
			{/each}
		</div>
	{/if}

	<div class="flex items-center gap-2">
		<button
			class="ui-link text-xs"
			onclick={addCondition}
			>+ {t('dataBrowser.filterPanel.addCondition')}</button
		>
		<div class="flex-1"></div>
		<button
			class="ui-btn ui-btn-ghost ui-btn-sm"
			onclick={handleClear}
		>
			{t('common.clear')}
		</button>
		<button
			class="ui-btn ui-btn-primary ui-btn-sm"
			onclick={handleApply}
		>
			{t('dataBrowser.filterPanel.apply')}
		</button>
	</div>
</div>
