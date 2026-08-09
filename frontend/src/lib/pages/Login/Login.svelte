<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { navigate } from 'svelte-routing';
  import { connectionStore } from '$stores/connection';

  let username = $state('root');
  let password = $state('');
  let rememberMe = $state(false);
  let isLoading = $state(false);
  let errorMsg = $state('');
  let formValid = $derived(username.trim() && password.trim());

  onMount(() => {
    connectionStore.loadSavedConnection();
    const saved = localStorage.getItem('graphdb_connection');
    if (saved) {
      try {
        const info = JSON.parse(saved);
        if (info.username) username = info.username;
        if (info.password) password = info.password;
        rememberMe = true;
      } catch { /* ignore */ }
    }
  });

  async function handleSubmit(e: Event) {
    e.preventDefault();
    if (!formValid) return;
    isLoading = true;
    errorMsg = '';
    try {
      await connectionStore.login(username, password, rememberMe);
      navigate('/');
    } catch (err) {
      errorMsg = err instanceof Error ? err.message : 'Login failed';
    } finally {
      isLoading = false;
    }
  }
</script>

<div class="min-h-screen flex items-center justify-center bg-gray-100 dark:bg-[#0B0F17] transition-colors duration-300">
  <div class="bg-white dark:bg-[#1C2333] rounded-lg shadow-md p-8 w-full max-w-sm transition-colors duration-300">
    <h1 class="text-2xl font-bold text-center text-gray-800 dark:text-gray-100 mb-6">{$t('header.title')}</h1>
    <form onsubmit={handleSubmit}>
      {#if errorMsg}
        <div class="mb-4 p-3 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 rounded text-red-600 dark:text-red-400 text-sm">{errorMsg}</div>
      {/if}
      <div class="mb-4">
        <label class="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-1" for="username">{$t('common.username')}</label>
        <input
          id="username"
          type="text"
          bind:value={username}
          class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200 rounded text-sm focus:outline-none focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
          placeholder="{$t('login.usernamePlaceholder')}"
          disabled={isLoading}
        />
      </div>
      <div class="mb-4">
        <label class="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-1" for="password">{$t('common.password')}</label>
        <input
          id="password"
          type="password"
          bind:value={password}
          class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200 rounded text-sm focus:outline-none focus:border-blue-500 focus:ring-1 focus:ring-blue-500"
          placeholder="{$t('login.passwordPlaceholder')}"
          disabled={isLoading}
        />
      </div>
      <div class="mb-4">
        <label class="flex items-center gap-2 text-sm text-gray-600 dark:text-gray-400">
          <input type="checkbox" bind:checked={rememberMe} class="rounded dark:bg-gray-700" />
          {$t('common.rememberMe')}
        </label>
      </div>
      <button
        type="submit"
        class="w-full py-2 px-4 bg-blue-500 hover:bg-blue-600 text-white font-medium rounded text-sm transition-colors disabled:opacity-50 disabled:cursor-not-allowed cursor-pointer"
        disabled={!formValid || isLoading}
      >
        {isLoading ? $t('common.loading') : $t('common.login')}
      </button>
    </form>
  </div>
</div>