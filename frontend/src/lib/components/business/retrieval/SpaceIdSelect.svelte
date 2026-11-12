<script lang="ts">
	import { t } from '$i18n';
	import { fromStore } from 'svelte/store';
	import { schemaStore } from '$stores/schema';

	/**
	 * Space picker for index-management forms. The form APIs take a numeric
	 * space id, but users think in space names, so the select is driven by
	 * the schema store's space list and binds the matching id.
	 */
	let {
		spaceId = $bindable(1),
		currentSpace = null,
	}: {
		spaceId?: number;
		currentSpace?: string | null;
	} = $props();

	const schemaState = fromStore(schemaStore);
	const spaces = $derived(schemaState.current.spaces);

	// Snap the selection to the active space (or the first available one)
	// until the user picks a different entry explicitly.
	$effect(() => {
		if (spaces.length === 0) return;
		if (spaces.some((s) => s.id === spaceId)) return;
		const active = spaces.find((s) => s.name === currentSpace) ?? spaces[0];
		spaceId = active.id;
	});
</script>

<select
	class="ui-input ui-input-sm"
	bind:value={spaceId}
	aria-label={t('vector.spaceId')}
>
	{#each spaces as space (space.id)}
		<option value={space.id}>{space.name} ({space.id})</option>
	{/each}
</select>
