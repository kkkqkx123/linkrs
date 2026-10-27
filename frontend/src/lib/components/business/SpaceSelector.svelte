<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from '$i18n';
	import { schemaStore } from '$stores/schema';

	let spaces = $state<Array<{ name: string }>>([]);
	let currentSpace = $state<string | null>(null);

	onMount(() => {
		schemaStore.fetchSpaces();
		const unsub1 = schemaStore.subscribe((s) => {
			spaces = s.spaces;
			currentSpace = s.currentSpace;
		});
		return unsub1;
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
