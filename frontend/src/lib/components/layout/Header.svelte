<script lang="ts">
  import { t } from 'svelte-i18n';
  import { connectionStore } from '$stores/connection';
  import { navigate } from 'svelte-routing';
  import SpaceSelector from '$components/business/SpaceSelector.svelte';
  import LanguageSwitcher from '$components/common/LanguageSwitcher.svelte';
  import ThemeToggle from '$components/common/ThemeToggle.svelte';
  import HealthMonitor from '$components/common/HealthMonitor.svelte';

  let store = $state({ isVerified: false, connectionInfo: { username: '' }, isLoading: false });
  connectionStore.subscribe(v => store = v);
</script>

<header class="h-14 bg-white dark:bg-[#1C2333] border-b border-gray-200 dark:border-gray-700/50 flex items-center justify-between px-6 flex-shrink-0 transition-colors duration-300">
  <div class="flex items-center gap-4">
    <span class="font-semibold text-gray-800 dark:text-gray-100">{$t('header.title')}</span>
    {#if store.isVerified}
      <div class="h-4 w-px bg-gray-300 dark:bg-gray-600"></div>
      <SpaceSelector />
    {/if}
  </div>
  <div class="flex items-center gap-4">
    <LanguageSwitcher />
    <ThemeToggle />
    <HealthMonitor />
    {#if store.isVerified}
      <span class="text-sm text-gray-600 dark:text-gray-400">👤 {store.connectionInfo.username}</span>
      <button
        class="px-3 py-1 text-sm text-gray-600 dark:text-gray-400 hover:text-red-500 hover:bg-red-50 dark:hover:bg-red-900/20 rounded transition-colors cursor-pointer"
        onclick={async () => {
          await connectionStore.logout();
          navigate('/login');
        }}
        disabled={store.isLoading}
      >
        {$t('common.logout')}
      </button>
    {/if}
  </div>
</header>