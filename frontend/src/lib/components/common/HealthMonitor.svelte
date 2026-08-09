<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { t } from 'svelte-i18n';
  import { connectionStore } from '$stores/connection';
  import { connectionService } from '$services/connection';
  import type { HealthResponse } from '$services/connection';

  let isConnected = $state(false);
  let isChecking = $state(false);
  let panelOpen = $state(false);
  let healthData = $state<HealthResponse | null>(null);
  let lastCheckTime = $state<string | null>(null);
  let latency = $state<number | null>(null);
  let checkError = $state<string | null>(null);
  let panelEl = $state<HTMLDivElement>();

  onMount(() => {
    const unsub = connectionStore.subscribe(s => {
      isConnected = s.isConnected && s.isVerified;
    });
    return unsub;
  });

  function handleClickOutside(e: MouseEvent) {
    if (panelEl && !panelEl.contains(e.target as Node)) {
      panelOpen = false;
    }
  }

  $effect(() => {
    if (panelOpen) {
      document.addEventListener('click', handleClickOutside);
    }
    return () => {
      document.removeEventListener('click', handleClickOutside);
    };
  });

  async function handleCheckHealth() {
    isChecking = true;
    checkError = null;
    const start = performance.now();
    try {
      const result = await connectionService.health();
      latency = Math.round(performance.now() - start);
      healthData = result;
      lastCheckTime = new Date().toLocaleTimeString();
      if (result.status !== 'healthy') {
        checkError = `Status: ${result.status}`;
      }
    } catch (err) {
      latency = Math.round(performance.now() - start);
      checkError = err instanceof Error ? err.message : 'Health check failed';
      healthData = null;
    } finally {
      isChecking = false;
    }
  }

  function togglePanel() {
    panelOpen = !panelOpen;
    if (panelOpen && !healthData) {
      handleCheckHealth();
    }
  }
</script>

<div class="relative" bind:this={panelEl}>
  <button
    class="flex items-center gap-1.5 px-2 py-1 rounded hover:bg-gray-100 dark:hover:bg-gray-700/50 transition-colors cursor-pointer text-sm"
    onclick={togglePanel}
    title="{$t('common.healthCheck')}"
  >
    <span
      class="inline-block w-2 h-2 rounded-full {isChecking ? 'bg-yellow-400 animate-pulse' : isConnected ? 'bg-green-500' : 'bg-red-500'}"
    ></span>
    <span class="hidden sm:inline text-gray-500 dark:text-gray-400">
      {isChecking ? $t('common.loading') : isConnected ? $t('common.connected') : $t('common.disconnected')}
    </span>
  </button>

  {#if panelOpen}
    <div class="absolute right-0 top-full mt-2 w-72 bg-white dark:bg-[#1C2333] rounded-lg shadow-lg border border-gray-200 dark:border-gray-700 z-50 animate-fade-in">
      <div class="p-4">
        <div class="flex items-center justify-between mb-3">
          <h3 class="font-semibold text-gray-800 dark:text-gray-100 text-sm">{$t('common.healthCheck')}</h3>
          <button
            class="px-3 py-1 text-xs bg-blue-500 hover:bg-blue-600 text-white rounded transition-colors disabled:opacity-50 cursor-pointer"
            onclick={handleCheckHealth}
            disabled={isChecking}
          >
            {isChecking ? $t('common.loading') : $t('common.refresh')}
          </button>
        </div>

        <div class="space-y-2 text-sm">
          <div class="flex justify-between items-center">
            <span class="text-gray-500 dark:text-gray-400">{$t('common.status')}</span>
            <span class="flex items-center gap-1.5">
              <span class="inline-block w-1.5 h-1.5 rounded-full {isConnected ? 'bg-green-500' : 'bg-red-500'}"></span>
              <span class="{isConnected ? 'text-green-600 dark:text-green-400' : 'text-red-600 dark:text-red-400'} font-medium">
                {isConnected ? $t('common.connected') : $t('common.disconnected')}
              </span>
            </span>
          </div>

          {#if healthData}
            <div class="flex justify-between">
              <span class="text-gray-500 dark:text-gray-400">{$t('common.service')}</span>
              <span class="text-gray-800 dark:text-gray-200 font-mono">{healthData.service}</span>
            </div>
            <div class="flex justify-between">
              <span class="text-gray-500 dark:text-gray-400">{$t('common.version')}</span>
              <span class="text-gray-800 dark:text-gray-200 font-mono">{healthData.version}</span>
            </div>
            <div class="flex justify-between">
              <span class="text-gray-500 dark:text-gray-400">{$t('common.healthStatus')}</span>
              <span class="font-medium {healthData.status === 'healthy' ? 'text-green-600 dark:text-green-400' : 'text-yellow-600 dark:text-yellow-400'}">
                {healthData.status}
              </span>
            </div>
          {/if}

          {#if latency !== null}
            <div class="flex justify-between">
              <span class="text-gray-500 dark:text-gray-400">{$t('common.latency')}</span>
              <span class="text-gray-800 dark:text-gray-200 font-mono {latency > 1000 ? 'text-yellow-600 dark:text-yellow-400' : latency > 500 ? 'text-orange-600 dark:text-orange-400' : ''}">
                {latency}ms
              </span>
            </div>
          {/if}

          {#if lastCheckTime}
            <div class="flex justify-between">
              <span class="text-gray-500 dark:text-gray-400">{$t('common.lastCheck')}</span>
              <span class="text-gray-500 dark:text-gray-400 text-xs">{lastCheckTime}</span>
            </div>
          {/if}

          {#if checkError}
            <div class="mt-2 p-2 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 rounded text-xs text-red-600 dark:text-red-400">
              {checkError}
            </div>
          {/if}
        </div>
      </div>
    </div>
  {/if}
</div>