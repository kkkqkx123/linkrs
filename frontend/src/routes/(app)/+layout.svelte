<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { fromStore, get } from 'svelte/store';
	import { page } from '$app/state';
	import { t } from '$i18n';
	import { isAuthenticated, connectionStore } from '$stores/connection';
	import { applyRouteDensity } from '$stores/ui';
	import LoadingScreen from '$components/common/LoadingScreen.svelte';
	import Header from '$components/layout/Header.svelte';
	import Sidebar from '$components/layout/Sidebar.svelte';

	let { children } = $props();

	let checking = $state(true);
	const authState = fromStore(isAuthenticated);

	// Canvas-like pages (graph, data browser) use the full working area;
	// form/config pages stay centered with a readable max width.
	const WIDE_ROUTES = ['/data-browser'];
	const wide = $derived(WIDE_ROUTES.some((r) => page.url.pathname.startsWith(r)));

	$effect(() => {
		applyRouteDensity(page.url.pathname);
	});

	onMount(async () => {
		if (!get(isAuthenticated)) {
			const state = get(connectionStore);
			if (state.isConnected && !state.isVerified) {
				await connectionStore.checkHealth();
			}
			if (!get(isAuthenticated)) {
				await goto('/login');
				return;
			}
		}
		checking = false;
	});
</script>

{#if checking}
	<LoadingScreen />
{:else if authState.current}
	<div
		class="flex h-screen overflow-hidden bg-page transition-colors duration-300"
	>
		<Sidebar />
		<div class="flex flex-col flex-1 min-w-0">
			<Header />
			<main
				class="flex-1 overflow-auto py-8 px-7 bg-page transition-colors duration-300"
			>
				<div class={wide ? 'w-full' : 'max-w-[1500px] mx-auto w-full'}>
					{@render children()}
				</div>
			</main>
		</div>
	</div>
{:else}
	<p class="p-6 text-sm text-muted">
		{t('common.redirecting')}
	</p>
{/if}
