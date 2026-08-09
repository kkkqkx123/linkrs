<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { isAuthenticated, connectionStore } from '$stores/connection';
  import LoadingScreen from '$components/common/LoadingScreen.svelte';

  let { children } = $props();
  let checking = $state(true);

  onMount(async () => {
    let auth = false;
    const unsub = isAuthenticated.subscribe(v => auth = v)();
    if (!auth) {
      let store = null!;
      const unsub2 = connectionStore.subscribe(s => store = s)();
      if (store.isConnected && !store.isVerified) {
        await connectionStore.checkHealth();
      }
    }
    checking = false;
  });
</script>

{#if checking}
  <LoadingScreen />
{:else if $isAuthenticated}
  {@render children()}
{:else}
  <a href="/login">{$t('common.redirecting')}</a>
{/if}