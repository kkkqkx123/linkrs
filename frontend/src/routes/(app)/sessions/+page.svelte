<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { get } from 'svelte/store';
	import { t } from '$i18n';
	import { canManageUsers } from '$stores/connection';
	import PageHeader from '$components/layout/PageHeader.svelte';
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

<div class="ui-page-doc animate-fade-in">
	<PageHeader title={t('sessions.title')}>
		<button
			class="ui-btn ui-btn-primary ui-btn-sm"
			onclick={loadSessions}
			disabled={loading}
		>
			{loading ? t('common.loading') : t('common.refresh')}
		</button>
	</PageHeader>

	{#if error}
		<p class="ui-error-text">{error}</p>
	{/if}

	<section class="ui-card shadow-sm p-6">
		{#if sessions.length === 0}
			<p class="text-sm text-gray-500 dark:text-gray-400">
				{t('sessions.empty')}
			</p>
		{:else}
			<div class="overflow-x-auto">
				<table class="ui-table w-full">
					<thead>
						<tr>
							<th>ID</th>
							<th>{t('common.username')}</th>
							<th>{t('users.space')}</th>
							<th>Graph</th>
							<th>{t('common.actions')}</th>
						</tr>
					</thead>
					<tbody>
						{#each sessions as session (session.session_id)}
							<tr>
								<td class="font-mono">{session.session_id}</td>
								<td class="font-mono">{session.username}</td>
								<td>{session.space_name ?? '—'}</td>
								<td>{session.graph_addr ?? '—'}</td>
								<td>
									<button
										class="ui-btn ui-btn-danger-outline ui-btn-xs"
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
