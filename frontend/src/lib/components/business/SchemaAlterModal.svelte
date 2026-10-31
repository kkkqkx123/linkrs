<script lang="ts">
	import { t } from '$i18n';
	import Modal from '$components/common/Modal.svelte';
	import { DATA_TYPE_LABELS } from '$config/constants';
	import type { components } from '$lib/api/schema';

	type PropertyDef = components['schemas']['PropertyDef'];

	let {
		title,
		properties,
		open,
		busy,
		onClose,
		onSubmit,
	}: {
		title: string;
		properties: PropertyDef[];
		open: boolean;
		busy: boolean;
		onClose: () => void;
		onSubmit: (added: PropertyDef[], dropped: string[]) => void;
	} = $props();

	let addName = $state('');
	let addType = $state('STRING');
	let pendingDrops = $state<string[]>([]);

	const dataTypes = Object.values(DATA_TYPE_LABELS).filter(Boolean);

	function toggleDrop(name: string) {
		if (pendingDrops.includes(name)) {
			pendingDrops = pendingDrops.filter((n) => n !== name);
		} else {
			pendingDrops = [...pendingDrops, name];
		}
	}

	function submit() {
		const added: PropertyDef[] = [];
		const trimmed = addName.trim();
		if (trimmed) {
			added.push({ name: trimmed, data_type: addType, nullable: true });
		}
		onSubmit(added, pendingDrops);
		addName = '';
		pendingDrops = [];
	}
</script>

<Modal {open} {title} {onClose}>
	<div class="space-y-2 max-h-48 overflow-y-auto mb-3">
		{#each properties as prop (prop.name)}
			<label
				class="flex items-center gap-2 text-sm text-ink cursor-pointer"
			>
				<input
					type="checkbox"
					checked={pendingDrops.includes(prop.name)}
					onchange={() => toggleDrop(prop.name)}
					class="cursor-pointer"
				/>
				<span class="font-mono text-xs">{prop.name}</span>
				<span class="text-xs text-muted">{prop.data_type}</span>
			</label>
		{/each}
		{#if properties.length === 0}
			<p class="text-xs text-muted">{t('common.noProperties')}</p>
		{/if}
	</div>
	<div class="flex gap-2 mb-2">
		<input
			type="text"
			bind:value={addName}
			class="ui-input flex-1"
			placeholder={t('common.name')}
			disabled={busy}
		/>
		<select bind:value={addType} class="ui-input" disabled={busy}>
			{#each dataTypes as dt (dt)}
				<option value={dt}>{dt}</option>
			{/each}
		</select>
	</div>
	<p class="text-xs text-muted mb-3">{t('schema.alterHint')}</p>
	{#snippet footer()}
		<button class="ui-btn ui-btn-ghost" onclick={onClose} disabled={busy}
			>{t('common.cancel')}</button
		>
		<button class="ui-btn ui-btn-primary" onclick={submit} disabled={busy}
			>{t('common.save')}</button
		>
	{/snippet}
</Modal>
