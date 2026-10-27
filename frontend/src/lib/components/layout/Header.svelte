<script lang="ts">
	import { t } from '$i18n';
	import { connectionStore } from '$stores/connection';
	import type { UiRole } from '$lib/auth/roles';
	import { goto } from '$app/navigation';
	import SpaceSelector from '$components/business/SpaceSelector.svelte';
	import LanguageSwitcher from '$components/common/LanguageSwitcher.svelte';
	import ThemeToggle from '$components/common/ThemeToggle.svelte';
	import HealthMonitor from '$components/common/HealthMonitor.svelte';
	import { User } from 'lucide-svelte';

	let store = $state({
		isVerified: false,
		connectionInfo: { username: '' },
		role: null as UiRole | null,
		mustChangePassword: false,
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
	class="bg-surface border-b border-line flex items-center justify-between px-7 flex-shrink-0 transition-colors duration-300"
	style="height: var(--comp-height-top)"
>
	<div class="flex items-center gap-3">
		<span class="text-base font-semibold text-ink"
			>{t('app.title')}</span
		>
		{#if store.isVerified}
			<div class="h-4 w-px bg-line"></div>
			<SpaceSelector />
		{/if}
	</div>
	<div class="flex items-center gap-3">
		<LanguageSwitcher />
		<ThemeToggle />
		<HealthMonitor />
		{#if store.isVerified}
			{#if store.mustChangePassword}
				<span
					class="ui-badge bg-warning-light text-warning border-warning/40"
				>
					{t('login.mustChangePassword')}
				</span>
			{/if}
			<span class="flex items-center gap-1.5 text-sm text-muted">
				<User size={16} />
				{store.connectionInfo.username}
			</span>
			{#if store.role}
				<span
					class="ui-badge bg-info-light text-info border-info/40"
				>
					{roleLabel(store.role)}
				</span>
			{/if}
			<button
				class="ui-btn ui-btn-ghost !px-3 !py-1"
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
