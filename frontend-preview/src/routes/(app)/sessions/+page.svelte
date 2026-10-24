<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { get } from 'svelte/store';
	import { t } from '$i18n';
	import { canManageUsers } from '$stores/connection';
	import {
		connectionService,
		type SessionListItem,
	} from '$services/connection';

	let sessions = $state<SessionListItem[]>([]);
	let loading = $state(false);
	let error = $state<string | null>(null);

	onMount(async () => {
		if (!get(canManageUsers)) {
			await goto('/');
			return;
		}
		await loadSessions();
	});

	async function loadSessions() {
		loading = true;
		error = null;
		try {
			const result = await connectionService.sessions.list();
			sessions = result.sessions;
		} catch (err) {
			error = err instanceof Error ? err.message : t('sessions.loadFailed');
		} finally {
			loading = false;
		}
	}

	async function terminateSession(id: number) {
		if (!confirm(t('sessions.confirmTerminate', { id: String(id) }))) return;
		loading = true;
		error = null;
		try {
			await connectionService.sessions.delete(id);
			await loadSessions();
		} catch (err) {
			error =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			loading = false;
		}
	}
</script>

<div class="max-w-6xl mx-auto space-y-4 animate-fade-in pb-8">
	<div class="flex items-center justify-between">
		<h1 class="text-xl font-bold text-gray-800 dark:text-gray-100">
			{t('sessions.title')}
		</h1>
		<button
			class="px-3 py-1 text-xs bg-blue-500 hover:bg-blue-600 text-white rounded transition-colors disabled:opacity-50 cursor-pointer"
			onclick={loadSessions}
			disabled={loading}
		>
			{loading ? t('common.loading') : t('common.refresh')}
		</button>
	</div>

	{#if error}
		<p class="text-xs text-red-500">{error}</p>
	{/if}

	<section
		class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
	>
		{#if sessions.length === 0}
			<p class="text-sm text-gray-500 dark:text-gray-400">{t('sessions.empty')}</p>
		{:else}
			<div class="overflow-x-auto">
				<table class="w-full text-sm">
					<thead>
						<tr class="text-left text-xs text-gray-500 dark:text-gray-400">
							<th class="py-2 pr-4">ID</th>
							<th class="py-2 pr-4">{t('common.username')}</th>
							<th class="py-2 pr-4">{t('users.space')}</th>
							<th class="py-2 pr-4">Graph</th>
							<th class="py-2">{t('common.actions')}</th>
						</tr>
					</thead>
					<tbody>
						{#each sessions as session (session.session_id)}
							<tr
								class="border-t border-gray-100 dark:border-gray-700/50 text-gray-700 dark:text-gray-200"
							>
								<td class="py-2 pr-4 font-mono">{session.session_id}</td>
								<td class="py-2 pr-4 font-mono">{session.username}</td>
								<td class="py-2 pr-4">{session.space_name ?? '—'}</td>
								<td class="py-2 pr-4">{session.graph_addr ?? '—'}</td>
								<td class="py-2">
									<button
										class="px-2 py-1 text-xs border border-red-300 dark:border-red-800 text-red-600 dark:text-red-400 rounded hover:bg-red-50 dark:hover:bg-red-900/20 cursor-pointer"
										onclick={() => terminateSession(session.session_id)}
										disabled={loading}
									>
										{t('sessions.terminate')}
									</button>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</section>
</div>
