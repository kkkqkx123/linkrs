<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { schemaStore } from '$stores/schema';

  let spaces = $state<Array<{ name: string }>>([]);
  let currentSpace = $state<string | null>(null);

  onMount(() => {
    schemaStore.fetchSpaces();
    const unsub1 = schemaStore.subscribe(s => {
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
  <span class="text-gray-500">{$t('sidebar.spaces')}:</span>
  <select
    class="px-2 py-1 border border-gray-300 rounded text-sm bg-white focus:outline-none focus:border-blue-500"
    value={currentSpace || ''}
    onchange={handleChange}
  >
    <option value="">-- {$t('common.select')} --</option>
    {#each spaces as space}
      <option value={space.name}>{space.name}</option>
    {/each}
  </select>
</div>