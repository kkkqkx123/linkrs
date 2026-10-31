<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { get } from 'svelte/store';
	import { t, type MessageKey } from '$i18n';
	import { canManageUsers } from '$stores/connection';
	import PageHeader from '$components/layout/PageHeader.svelte';
	import ConfirmDialog from '$components/common/ConfirmDialog.svelte';
	import {
		connectionService,
		type SessionListItem,
	} from '$services/connection';
	import { notificationStore } from '$stores/notification';
	
	let sessions = $state<SessionListItem[]>([]);
	let loading = $state(false);

	function reportError(err: unknown, key: MessageKey = 'notification.requestFailed') {
		notificationStore.error(
			key,
			undefined,
			err instanceof Error ? err.message : undefined,
		);
	}

	// Pending confirmation; the dialog owns the decision, the action runs only
	// after explicit confirmation.
	let pendingConfirm = $state<{
		title: string;
		message: string;
		run: () => Promise<void>;
	} | null>(null);

	async function confirmPending() {
		const pending = pendingConfirm;
		pendingConfirm = null;
		if (pending) await pending.run();
	}

	onMount(async () => {
		if (!get(canManageUsers)) {
			await goto('/');
			return;
		}
		await loadSessions();
	});

	async function loadSessions() {
		loading = true;
		try {
			const result = await connectionService.sessions.list();
			sessions = result.sessions;
		} catch (err) {
			reportError(err, 'sessions.loadFailed');
		} finally {
			loading = false;
		}
	}

	async function terminateSession(id: number) {
		pendingConfirm = {
			title: t('sessions.terminate'),
			message: t('sessions.confirmTerminate', { id: String(id) }),
			run: async () => {
				loading = true;
				try {
					await connectionService.sessions.delete(id);
					await loadSessions();
				} catch (err) {
					reportError(err);
				} finally {
					loading = false;
				}
			},
		};
	}
</script>

<div class="ui-page-doc animate-fade-in">
	<PageHeader title={t('sessions.title')}>
		<button class="ui-btn ui-btn-primary ui-btn-sm" onclick={loadSessions} disabled={loading}>
			{loading ? t('common.loading') : t('common.refresh')}
		</button>
	</PageHeader>

	<section class="ui-card shadow-sm p-6">
		{#if sessions.length === 0}
			<p class="text-sm text-muted">
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

<ConfirmDialog
	open={pendingConfirm !== null}
	title={pendingConfirm?.title ?? ''}
	message={pendingConfirm?.message ?? ''}
	danger
	busy={loading}
	onConfirm={confirmPending}
	onClose={() => (pendingConfirm = null)}
/>
