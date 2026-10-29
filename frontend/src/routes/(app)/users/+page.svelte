<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { get } from 'svelte/store';
	import { t, type MessageKey } from '$i18n';
	import { canManageUsers } from '$stores/connection';
	import { notificationStore } from '$stores/notification';
	import PageHeader from '$components/layout/PageHeader.svelte';
	import ConfirmDialog from '$components/common/ConfirmDialog.svelte';
	import { usersService, type ManagedUser } from '$services/users';

	let users = $state<ManagedUser[]>([]);
	let fallback = $state(false);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let query = $state('');
	let newUsername = $state('');
	let newPassword = $state('');
	let resetTarget = $state<string | null>(null);
	let resetPassword = $state('');
	let roleTarget = $state<string | null>(null);
	let roleSpace = $state('');
	let roleName = $state('USER');
	let ownOldPassword = $state('');
	let ownNewPassword = $state('');
	let ownConfirmPassword = $state('');

	// Pending confirmation; the dialog owns the decision, the action runs only
	// after explicit confirmation.
	let pendingConfirm = $state<{
		title: string;
		message: string;
		run: () => Promise<void>;
	} | null>(null);

	const filtered = $derived(
		users.filter((user) =>
			user.username.toLowerCase().includes(query.trim().toLowerCase()),
		),
	);

	onMount(async () => {
		if (!get(canManageUsers)) {
			await goto('/');
			return;
		}
		await loadUsers();
	});

	/** Shared wrapper: one loading/error path for every mutating action, with an
	 * optional success toast so success and failure feedback stay symmetric. */
	async function runAction(action: () => Promise<void>, successKey?: MessageKey) {
		loading = true;
		error = null;
		try {
			await action();
			if (successKey) notificationStore.success(successKey);
		} catch (err) {
			error = err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			loading = false;
		}
	}

	async function loadUsers() {
		await runAction(async () => {
			const result = await usersService.list();
			users = result.users;
			fallback = result.fallback;
		});
	}

	async function createUser() {
		if (!newUsername.trim() || !newPassword.trim()) return;
		await runAction(async () => {
			await usersService.create(newUsername.trim(), newPassword);
			newUsername = '';
			newPassword = '';
			await loadUsers();
		}, 'users.createdSuccess');
	}

	function requestToggle(user: ManagedUser, enabled: boolean) {
		if (enabled) {
			void runAction(
				() => usersService.setEnabled(user.username, true).then(() => loadUsers()),
				'users.statusSuccess',
			);
			return;
		}
		// Disabling locks the user out; ask first.
		pendingConfirm = {
			title: t('users.disable'),
			message: t('users.confirmDisable', { name: user.username }),
			run: async () => {
				await usersService.setEnabled(user.username, false);
				await loadUsers();
			},
		};
	}

	function requestDeleteUser(user: ManagedUser) {
		pendingConfirm = {
			title: t('users.deleteUser'),
			message: t('users.confirmDeleteUser', { name: user.username }),
			run: async () => {
				await usersService.drop(user.username);
				await loadUsers();
			},
		};
	}

	async function confirmPending() {
		const pending = pendingConfirm;
		if (!pending) return;
		pendingConfirm = null;
		await runAction(pending.run, 'users.statusSuccess');
	}

	async function submitReset(user: ManagedUser) {
		if (!resetPassword.trim()) return;
		await runAction(async () => {
			await usersService.resetPassword(user.username, resetPassword);
			resetTarget = null;
			resetPassword = '';
			await loadUsers();
		}, 'users.resetSuccess');
	}

	async function submitGrant(user: ManagedUser) {
		if (!roleSpace.trim() || !roleName.trim()) return;
		await runAction(async () => {
			await usersService.grant(user.username, roleName.trim(), roleSpace.trim());
			roleTarget = null;
			await loadUsers();
		}, 'users.roleSuccess');
	}

	async function submitRevoke(user: ManagedUser) {
		if (!roleSpace.trim() || !roleName.trim()) return;
		await runAction(async () => {
			await usersService.revoke(user.username, roleName.trim(), roleSpace.trim());
			roleTarget = null;
			await loadUsers();
		}, 'users.roleSuccess');
	}

	async function submitOwnPassword() {
		if (!ownOldPassword.trim() || !ownNewPassword.trim()) return;
		if (ownNewPassword !== ownConfirmPassword) {
			error = t('users.passwordMismatch');
			return;
		}
		await runAction(async () => {
			await usersService.changeOwnPassword(ownOldPassword, ownNewPassword);
			ownOldPassword = '';
			ownNewPassword = '';
			ownConfirmPassword = '';
		}, 'users.passwordChangedSuccess');
	}
</script>

<div class="ui-page-doc animate-fade-in">
	<PageHeader title={t('users.title')}>
		<button
			class="ui-btn ui-btn-primary ui-btn-sm"
			onclick={loadUsers}
			disabled={loading}
		>
			{loading ? t('common.loading') : t('common.refresh')}
		</button>
	</PageHeader>

	{#if fallback}
		<p class="ui-alert ui-alert-warn">
			{t('users.queryFallback')}
		</p>
	{/if}

	{#if error}
		<p class="ui-error-text">{error}</p>
	{/if}

	<section class="ui-card shadow-sm p-6">
		<h2 class="ui-section-title">
			{t('users.create')}
		</h2>
		<div class="flex flex-wrap gap-2 text-sm">
			<input
				class="ui-input ui-input-sm"
				placeholder={t('common.username')}
				bind:value={newUsername}
				disabled={loading}
			/>
			<input
				type="password"
				class="ui-input ui-input-sm"
				placeholder={t('common.password')}
				bind:value={newPassword}
				disabled={loading}
			/>
			<button
				class="ui-btn ui-btn-primary ui-btn-sm"
				onclick={createUser}
				disabled={loading || !newUsername.trim() || !newPassword.trim()}
			>
				{t('common.create')}
			</button>
		</div>
	</section>

	<section class="ui-card shadow-sm p-6">
		<h2 class="ui-section-title">
			{t('users.changeOwnPassword')}
		</h2>
		<div class="flex flex-wrap gap-2 text-sm">
			<input
				type="password"
				class="ui-input ui-input-sm"
				placeholder={t('users.oldPassword')}
				bind:value={ownOldPassword}
				disabled={loading}
			/>
			<input
				type="password"
				class="ui-input ui-input-sm"
				placeholder={t('users.newPassword')}
				bind:value={ownNewPassword}
				disabled={loading}
			/>
			<input
				type="password"
				class="ui-input ui-input-sm"
				placeholder={t('users.confirmPassword')}
				bind:value={ownConfirmPassword}
				disabled={loading}
			/>
			<button
				class="ui-btn ui-btn-primary ui-btn-sm"
				onclick={submitOwnPassword}
				disabled={loading || !ownOldPassword.trim() || !ownNewPassword.trim()}
			>
				{t('common.save')}
			</button>
		</div>
	</section>

	<section class="ui-card shadow-sm p-6">
		<div class="mb-3">
			<input
				class="ui-input w-full"
				placeholder={t('users.search')}
				bind:value={query}
			/>
		</div>
		{#if filtered.length === 0}
			<p class="text-sm text-muted">{t('users.empty')}</p>
		{:else}
			<div class="overflow-x-auto">
				<table class="ui-table w-full">
					<thead>
						<tr>
							<th>{t('common.username')}</th>
							<th>{t('users.role')}</th>
							<th>{t('users.status')}</th>
							<th>{t('users.lastActive')}</th>
							<th>{t('common.actions')}</th>
						</tr>
					</thead>
					<tbody>
						{#each filtered as user (user.username)}
							<tr>
								<td class="font-mono">{user.username}</td>
								<td>{user.role ?? '—'}</td>
								<td>
									{user.status ?? (fallback ? '—' : t('users.enabled'))}
								</td>
								<td>{user.lastActive ?? '—'}</td>
								<td>
									<div class="flex flex-wrap gap-2">
										<button
											class="ui-btn ui-btn-ghost ui-btn-sm"
											onclick={() =>
												(resetTarget =
													resetTarget === user.username ? null : user.username)}
										>
											{t('users.resetPassword')}
										</button>
										{#if !fallback}
											<button
												class="ui-btn ui-btn-ghost ui-btn-sm"
												onclick={() => requestToggle(user, false)}
												disabled={loading}
											>
												{t('users.disable')}
											</button>
											<button
												class="ui-btn ui-btn-ghost ui-btn-sm"
												onclick={() => requestToggle(user, true)}
												disabled={loading}
											>
												{t('users.enable')}
											</button>
										{/if}
										<button
											class="ui-btn ui-btn-ghost ui-btn-sm"
											onclick={() =>
												(roleTarget =
													roleTarget === user.username ? null : user.username)}
										>
											{t('users.grant')}
										</button>
										<button
											class="ui-btn ui-btn-danger-outline ui-btn-xs"
											onclick={() => requestDeleteUser(user)}
											disabled={loading}
										>
											{t('users.deleteUser')}
										</button>
									</div>
									{#if roleTarget === user.username}
										<div class="mt-2 flex flex-wrap gap-2">
											<input
												class="ui-input ui-input-sm"
												placeholder={t('users.space')}
												bind:value={roleSpace}
												disabled={loading}
											/>
											<input
												class="ui-input ui-input-sm"
												placeholder={t('users.role')}
												bind:value={roleName}
												disabled={loading}
											/>
											<button
												class="ui-btn ui-btn-primary ui-btn-sm"
												onclick={() => submitGrant(user)}
												disabled={loading ||
													!roleSpace.trim() ||
													!roleName.trim()}
											>
												{t('users.grant')}
											</button>
											<button
												class="ui-btn ui-btn-ghost ui-btn-sm"
												onclick={() => submitRevoke(user)}
												disabled={loading ||
													!roleSpace.trim() ||
													!roleName.trim()}
											>
												{t('users.revoke')}
											</button>
										</div>
									{/if}
									{#if resetTarget === user.username}
										<div class="mt-2 flex gap-2">
											<input
												type="password"
												class="ui-input ui-input-sm"
												placeholder={t('common.password')}
												bind:value={resetPassword}
												disabled={loading}
											/>
											<button
												class="ui-btn ui-btn-primary ui-btn-sm"
												onclick={() => submitReset(user)}
												disabled={loading || !resetPassword.trim()}
											>
												{t('common.save')}
											</button>
										</div>
									{/if}
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
