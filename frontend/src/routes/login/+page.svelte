<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from '$i18n';
	import { goto } from '$app/navigation';
	import { connectionStore } from '$stores/connection';
	import { connectionService } from '$services/connection';
	import { ApiError } from '$lib/api/client';

	let username = $state('');
	let password = $state('');
	let rememberMe = $state(false);
	let isLoading = $state(false);
	let errorMsg = $state('');
	let formValid = $derived(username.trim() && password.trim());
	let serverService = $state<string | null>(null);
	let serverVersion = $state<string | null>(null);
	let serverStatus = $state<string | null>(null);
	let serverUnreachable = $state(false);

	onMount(() => {
		connectionStore.loadSavedConnection();
		const { connectionInfo, rememberMe: remembered } = get(connectionStore);
		username = connectionInfo.username;
		password = '';
		rememberMe = remembered;
		void loadServerInfo();
	});

	async function loadServerInfo() {
		try {
			const health = await connectionService.health();
			serverService =
				typeof health.service === 'string' ? health.service : null;
			serverVersion =
				typeof health.version === 'string' ? health.version : null;
			serverStatus = typeof health.status === 'string' ? health.status : null;
			serverUnreachable = false;
		} catch {
			serverUnreachable = true;
		}
	}

	async function handleSubmit(e: Event) {
		e.preventDefault();
		if (!formValid) return;
		isLoading = true;
		errorMsg = '';
		try {
			await connectionStore.login(username, password, rememberMe);
			goto('/');
		} catch (err) {
			if (err instanceof ApiError && err.status === 403) {
				errorMsg = err.message || t('login.loginFailed');
			} else {
				errorMsg = err instanceof Error ? err.message : t('login.loginFailed');
			}
		} finally {
			isLoading = false;
		}
	}
</script>

<div
	class="min-h-screen flex items-center justify-center bg-page transition-colors duration-300"
>
	<div class="ui-card shadow-lg p-8 w-full max-w-[380px] transition-colors duration-300">
		<h1
			class="text-xl font-bold text-center text-ink mb-1.5"
		>
			{t('app.title')}
		</h1>
		<p class="text-xs text-center text-muted mb-8">
			Linkrs Studio
		</p>
		<form onsubmit={handleSubmit}>
			{#if errorMsg}
				<div
					class="ui-alert ui-alert-error mb-5 text-sm"
				>
					{errorMsg}
				</div>
			{/if}
			<div class="mb-5">
				<label
					class="block text-sm font-medium text-ink mb-1.5"
					for="username">{t('common.username')}</label
				>
				<input
					id="username"
					type="text"
					bind:value={username}
					class="ui-input w-full"
					placeholder={t('login.usernamePlaceholder')}
					disabled={isLoading}
				/>
			</div>
			<div class="mb-5">
				<label
					class="block text-sm font-medium text-ink mb-1.5"
					for="password">{t('common.password')}</label
				>
				<input
					id="password"
					type="password"
					bind:value={password}
					class="ui-input w-full"
					placeholder={t('login.passwordPlaceholder')}
					disabled={isLoading}
				/>
			</div>
			<div class="mb-6">
				<label
					class="flex items-center gap-2 text-sm text-muted"
				>
					<input
						type="checkbox"
						bind:checked={rememberMe}
						class="rounded bg-surface"
					/>
					{t('common.rememberMe')}
				</label>
			</div>
			<button
				type="submit"
				class="ui-btn ui-btn-primary w-full"
				disabled={!formValid || isLoading}
			>
				{isLoading ? t('common.loading') : t('common.login')}
			</button>
		</form>
		<div
			class="mt-6 pt-4 border-t border-line text-xs text-muted"
		>
			<div class="flex items-center justify-between mb-1">
				<span>{t('common.healthCheck')}</span>
				{#if serverUnreachable}
					<span class="text-error">{t('common.disconnected')}</span>
				{:else if serverStatus}
					<span
						class={serverStatus === 'healthy'
							? 'text-success'
							: 'text-warning'}
					>
						{serverStatus === 'healthy'
							? t('common.healthy')
							: t('common.unhealthy')}
					</span>
				{:else}
					<span>{t('common.loading')}</span>
				{/if}
			</div>
			{#if serverService || serverVersion}
				<div class="flex items-center justify-between">
					<span>{t('common.service')}</span>
					<span class="font-mono text-ink"
						>{serverService ?? '—'}</span
					>
				</div>
				<div class="flex items-center justify-between">
					<span>{t('common.version')}</span>
					<span class="font-mono text-ink"
						>{serverVersion ?? '—'}</span
					>
				</div>
			{/if}
		</div>
	</div>
</div>
