<script lang="ts">
	import { t } from '$i18n';
	import { connectionStore } from '$stores/connection';
	import type { UiRole } from '$lib/auth/roles';
	import { goto } from '$app/navigation';
	import SpaceSelector from '$components/business/SpaceSelector.svelte';
	import LanguageSwitcher from '$components/common/LanguageSwitcher.svelte';
	import ThemeToggle from '$components/common/ThemeToggle.svelte';
	import HealthMonitor from '$components/common/HealthMonitor.svelte';
	import { toggleDensity, density } from '$stores/ui';
	import { fromStore } from 'svelte/store';
	import { Rows3, User } from '@lucide/svelte';

	const densityState = fromStore(density);
	function densityLabel(d: 'comfortable' | 'dense'): string {
		return d === 'dense'
			? t('ui.density.dense')
			: t('ui.density.comfortable');
	}

	const store = fromStore(connectionStore);

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
		{#if store.current.isVerified}
			<div class="h-4 w-px bg-line"></div>
			<SpaceSelector />
		{/if}
	</div>
	<div class="flex items-center gap-3">
		<LanguageSwitcher />
		<ThemeToggle />
		<HealthMonitor />
		<button
			type="button"
			class="ui-icon-btn"
			title={densityLabel(densityState.current)}
			aria-label={densityLabel(densityState.current)}
			onclick={toggleDensity}
		>
			<Rows3 size={16} />
		</button>
		{#if store.current.isVerified}
			{#if store.current.mustChangePassword}
				<span
					class="ui-badge bg-warning-light text-warning border-warning/40"
				>
					{t('login.mustChangePassword')}
				</span>
			{/if}
			<span class="flex items-center gap-1.5 text-sm text-muted">
				<User size={16} />
				{store.current.connectionInfo.username}
			</span>
			{#if store.current.role}
				<span
					class="ui-badge bg-info-light text-info border-info/40"
				>
					{roleLabel(store.current.role)}
				</span>
			{/if}
			<button
				class="ui-btn ui-btn-ghost !px-3 !py-1"
				onclick={async () => {
					await connectionStore.logout();
					goto('/login');
				}}
				disabled={store.current.isLoading}
			>
				{t('common.logout')}
			</button>
		{/if}
	</div>
</header>
