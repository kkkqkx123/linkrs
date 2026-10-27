<script lang="ts">
	import { t } from '$i18n';

	interface Props {
		space: string;
		label: string;
		fromVersion: number;
		toVersion: number;
		isEdge: boolean;
		expandContract: boolean;
		loading: boolean;
		streaming: boolean;
		onCreatePlan: () => void;
		onStatus: () => void;
		onHistory: () => void;
		onStream: () => void;
		onStopStream: () => void;
	}

	let {
		space = $bindable(),
		label = $bindable(),
		fromVersion = $bindable(),
		toVersion = $bindable(),
		isEdge = $bindable(),
		expandContract = $bindable(),
		loading,
		streaming,
		onCreatePlan,
		onStatus,
		onHistory,
		onStream,
		onStopStream,
	}: Props = $props();

	const inputClass =
		'ui-input ui-input-sm font-mono';
</script>

<section
	class="ui-card shadow-sm p-6"
>
	<h2 class="text-lg font-semibold text-ink mb-4">
		{t('migration.plan')}
	</h2>
	<div class="grid md:grid-cols-4 gap-2 text-sm mb-3">
		<label class="ui-field">
			{t('migration.space')}
			<input class={inputClass} bind:value={space} />
		</label>
		<label class="ui-field">
			{t('migration.label')}
			<input class={inputClass} bind:value={label} />
		</label>
		<label class="ui-field">
			{t('migration.fromVersion')}
			<input type="number" class={inputClass} bind:value={fromVersion} />
		</label>
		<label class="ui-field">
			{t('migration.toVersion')}
			<input type="number" class={inputClass} bind:value={toVersion} />
		</label>
	</div>
	<div class="flex flex-wrap items-center gap-4 text-xs text-muted mb-3">
		<label class="flex items-center gap-1">
			<input type="checkbox" bind:checked={isEdge} />
			{t('migration.isEdge')}
		</label>
		<label class="flex items-center gap-1">
			<input type="checkbox" bind:checked={expandContract} />
			{t('migration.expandContract')}
		</label>
	</div>
	<div class="flex flex-wrap gap-2">
		<button
			class="ui-btn ui-btn-primary ui-btn-sm"
			onclick={onCreatePlan}
			disabled={loading}
		>
			{t('migration.createPlan')}
		</button>
		<button
			class="ui-btn ui-btn-ghost ui-btn-sm"
			onclick={onStatus}
			disabled={loading}
		>
			{t('common.status')}
		</button>
		<button
			class="ui-btn ui-btn-ghost ui-btn-sm"
			onclick={onHistory}
			disabled={loading}
		>
			{t('migration.history')}
		</button>
		{#if streaming}
			<button
				class="ui-btn ui-btn-danger ui-btn-sm"
				onclick={onStopStream}
			>
				{t('common.cancel')}
			</button>
		{:else}
			<button
				class="ui-btn ui-btn-ghost ui-btn-sm"
				onclick={onStream}
				disabled={loading}
			>
				{t('migration.stream')}
			</button>
		{/if}
	</div>
</section>
