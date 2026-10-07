<script lang="ts">
	import { t } from '$i18n';
	import { connectionStore } from '$stores/connection';
	import type { UiRole } from '$lib/auth/roles';
	import { goto } from '$app/navigation';
	import SpaceSelector from '$components/business/SpaceSelector.svelte';
	import LanguageSwitcher from '$components/common/LanguageSwitcher.svelte';
	import ThemeToggle from '$components/common/ThemeToggle.svelte';
	import HealthMonitor from '$components/common/HealthMonitor.svelte';

	let store = $state({
		isVerified: false,
		connectionInfo: { username: '' },
		role: null as UiRole | null,
		isLoading: false,
	});
	connectionStore.subscribe((v) => (store = v));

	function roleLabel(role: UiRole | null): string {
		if (role === 'admin') return t('auth.role.admin');
		if (role === 'operator') return t('auth.role.operator');
		if (role === 'viewer') return t('auth.role.viewer');
		return '';
	}
</script>

<header
	class="h-14 bg-white dark:bg-[#1C2333] border-b border-gray-200 dark:border-gray-700/50 flex items-center justify-between px-6 flex-shrink-0 transition-colors duration-300"
>
	<div class="flex items-center gap-4">
		<span class="font-semibold text-gray-800 dark:text-gray-100"
			>{t('app.title')}</span
		>
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
			<span class="text-sm text-gray-600 dark:text-gray-400"
				>👤 {store.connectionInfo.username}</span
			>
			{#if store.role}
				<span
					class="px-2 py-0.5 text-xs rounded-full bg-blue-50 dark:bg-blue-900/30 text-blue-600 dark:text-blue-300 border border-blue-200 dark:border-blue-800"
				>
					{roleLabel(store.role)}
				</span>
			{/if}
			<button
				class="px-3 py-1 text-sm text-gray-600 dark:text-gray-400 hover:text-red-500 hover:bg-red-50 dark:hover:bg-red-900/20 rounded transition-colors cursor-pointer"
				onclick={async () => {
					await connectionStore.logout();
					goto('/login');
				}}
				disabled={store.isLoading}
			>
				{t('common.logout')}
			</button>
		{/if}
	</div>
</header>
