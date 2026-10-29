<script lang="ts">
	import { onMount } from 'svelte';
	import { fromStore } from 'svelte/store';
	import { t } from '$i18n';
	import { schemaStore } from '$stores/schema';

	const schema = fromStore(schemaStore);
	const spaces = $derived(schema.current.spaces);
	const currentSpace = $derived(schema.current.currentSpace);

	onMount(() => {
		schemaStore.fetchSpaces();
	});

	function handleChange(e: Event) {
		const select = e.target as HTMLSelectElement;
		schemaStore.setCurrentSpace(select.value || null);
	}
</script>

<div class="flex items-center gap-2 text-sm">
	<span class="text-muted">{t('sidebar.spaces')}:</span>
	<select
		class="ui-input ui-input-sm w-auto"
		value={currentSpace || ''}
		onchange={handleChange}
	>
		<option value="">-- {t('common.select')} --</option>
		{#each spaces as space (space.name)}
			<option value={space.name}>{space.name}</option>
		{/each}
	</select>
</div>
