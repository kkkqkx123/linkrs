<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { get } from 'svelte/store';
	import { t } from '$i18n';
	import { canManageUsers } from '$stores/connection';
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

	async function loadUsers() {
		loading = true;
		error = null;
		try {
			const result = await usersService.list();
			users = result.users;
			fallback = result.fallback;
		} catch (err) {
			error = err instanceof Error ? err.message : t('users.loadFailed');
		} finally {
			loading = false;
		}
	}

	async function createUser() {
		if (!newUsername.trim() || !newPassword.trim()) return;
		loading = true;
		error = null;
		try {
			await usersService.create(newUsername.trim(), newPassword);
			newUsername = '';
			newPassword = '';
			await loadUsers();
		} catch (err) {
			error =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			loading = false;
		}
	}

	async function toggleEnabled(user: ManagedUser, enabled: boolean) {
		if (
			!confirm(t('users.confirmDisable', { name: user.username })) &&
			!enabled
		) {
			return;
		}
		loading = true;
		error = null;
		try {
			await usersService.setEnabled(user.username, enabled);
			await loadUsers();
		} catch (err) {
			error =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			loading = false;
		}
	}

	async function submitReset(user: ManagedUser) {
		if (!resetPassword.trim()) return;
		loading = true;
		error = null;
		try {
			await usersService.resetPassword(user.username, resetPassword);
			resetTarget = null;
			resetPassword = '';
			await loadUsers();
		} catch (err) {
			error =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			loading = false;
		}
	}

	async function submitGrant(user: ManagedUser) {
		if (!roleSpace.trim() || !roleName.trim()) return;
		loading = true;
		error = null;
		try {
			await usersService.grant(user.username, roleName.trim(), roleSpace.trim());
			roleTarget = null;
			await loadUsers();
		} catch (err) {
			error =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			loading = false;
		}
	}

	async function submitRevoke(user: ManagedUser) {
		if (!roleSpace.trim() || !roleName.trim()) return;
		loading = true;
		error = null;
		try {
			await usersService.revoke(
				user.username,
				roleName.trim(),
				roleSpace.trim(),
			);
			roleTarget = null;
			await loadUsers();
		} catch (err) {
			error =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			loading = false;
		}
	}

	async function deleteUser(user: ManagedUser) {
		if (!confirm(t('users.confirmDeleteUser', { name: user.username }))) return;
		loading = true;
		error = null;
		try {
			await usersService.drop(user.username);
			await loadUsers();
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
			{t('users.title')}
		</h1>
		<button
			class="px-3 py-1 text-xs bg-blue-500 hover:bg-blue-600 text-white rounded transition-colors disabled:opacity-50 cursor-pointer"
			onclick={loadUsers}
			disabled={loading}
		>
			{loading ? t('common.loading') : t('common.refresh')}
		</button>
	</div>

	{#if fallback}
		<p
			class="p-3 bg-yellow-50 dark:bg-yellow-900/20 border border-yellow-200 dark:border-yellow-800 rounded text-xs text-yellow-700 dark:text-yellow-300"
		>
			{t('users.queryFallback')}
		</p>
	{/if}

	{#if error}
		<p class="text-xs text-red-500">{error}</p>
	{/if}

	<section
		class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
	>
		<h2 class="font-semibold text-gray-800 dark:text-gray-100 mb-3 text-sm">
			{t('users.create')}
		</h2>
		<div class="flex flex-wrap gap-2 text-sm">
			<input
				class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
				placeholder={t('common.username')}
				bind:value={newUsername}
				disabled={loading}
			/>
			<input
				type="password"
				class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
				placeholder={t('common.password')}
				bind:value={newPassword}
				disabled={loading}
			/>
			<button
				class="px-3 py-1 text-xs bg-blue-500 hover:bg-blue-600 text-white rounded transition-colors disabled:opacity-50 cursor-pointer"
				onclick={createUser}
				disabled={loading || !newUsername.trim() || !newPassword.trim()}
			>
				{t('common.create')}
			</button>
		</div>
	</section>

	<section
		class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
	>
		<div class="mb-3">
			<input
				class="w-full px-2 py-1 text-sm border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
				placeholder={t('users.search')}
				bind:value={query}
			/>
		</div>
		{#if filtered.length === 0}
			<p class="text-sm text-gray-500 dark:text-gray-400">{t('users.empty')}</p>
		{:else}
			<div class="overflow-x-auto">
				<table class="w-full text-sm">
					<thead>
						<tr class="text-left text-xs text-gray-500 dark:text-gray-400">
							<th class="py-2 pr-4">{t('common.username')}</th>
							<th class="py-2 pr-4">{t('users.role')}</th>
							<th class="py-2 pr-4">{t('users.status')}</th>
							<th class="py-2 pr-4">{t('users.lastActive')}</th>
							<th class="py-2">{t('common.actions')}</th>
						</tr>
					</thead>
					<tbody>
						{#each filtered as user (user.username)}
							<tr
								class="border-t border-gray-100 dark:border-gray-700/50 text-gray-700 dark:text-gray-200"
							>
								<td class="py-2 pr-4 font-mono">{user.username}</td>
								<td class="py-2 pr-4">{user.role ?? '—'}</td>
								<td class="py-2 pr-4">
									{user.status ?? (fallback ? '—' : t('users.enabled'))}
								</td>
								<td class="py-2 pr-4">{user.lastActive ?? '—'}</td>
								<td class="py-2">
									<div class="flex flex-wrap gap-2">
										<button
											class="px-2 py-1 text-xs border border-gray-300 dark:border-gray-600 rounded hover:bg-gray-50 dark:hover:bg-gray-700/50 cursor-pointer"
											onclick={() =>
												(resetTarget =
													resetTarget === user.username ? null : user.username)}
										>
											{t('users.resetPassword')}
										</button>
										{#if !fallback}
											<button
												class="px-2 py-1 text-xs border border-gray-300 dark:border-gray-600 rounded hover:bg-gray-50 dark:hover:bg-gray-700/50 cursor-pointer"
												onclick={() => toggleEnabled(user, false)}
												disabled={loading}
											>
												{t('users.disable')}
											</button>
											<button
												class="px-2 py-1 text-xs border border-gray-300 dark:border-gray-600 rounded hover:bg-gray-50 dark:hover:bg-gray-700/50 cursor-pointer"
												onclick={() => toggleEnabled(user, true)}
												disabled={loading}
											>
												{t('users.enable')}
											</button>
										{/if}
										<button
											class="px-2 py-1 text-xs border border-gray-300 dark:border-gray-600 rounded hover:bg-gray-50 dark:hover:bg-gray-700/50 cursor-pointer"
											onclick={() =>
												(roleTarget =
													roleTarget === user.username ? null : user.username)}
										>
											{t('users.grant')}
										</button>
										<button
											class="px-2 py-1 text-xs border border-red-300 dark:border-red-800 text-red-600 dark:text-red-400 rounded hover:bg-red-50 dark:hover:bg-red-900/20 cursor-pointer"
											onclick={() => deleteUser(user)}
											disabled={loading}
										>
											{t('users.deleteUser')}
										</button>
									</div>
									{#if roleTarget === user.username}
										<div class="mt-2 flex flex-wrap gap-2">
											<input
												class="px-2 py-1 text-xs border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-[#1C2333]"
												placeholder={t('users.space')}
												bind:value={roleSpace}
												disabled={loading}
											/>
											<input
												class="px-2 py-1 text-xs border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-[#1C2333]"
												placeholder={t('users.role')}
												bind:value={roleName}
												disabled={loading}
											/>
											<button
												class="px-2 py-1 text-xs bg-blue-500 hover:bg-blue-600 text-white rounded cursor-pointer"
												onclick={() => submitGrant(user)}
												disabled={loading ||
													!roleSpace.trim() ||
													!roleName.trim()}
											>
												{t('users.grant')}
											</button>
											<button
												class="px-2 py-1 text-xs border border-gray-300 dark:border-gray-600 rounded hover:bg-gray-50 dark:hover:bg-gray-700/50 cursor-pointer"
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
												class="px-2 py-1 text-xs border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-[#1C2333]"
												placeholder={t('common.password')}
												bind:value={resetPassword}
												disabled={loading}
											/>
											<button
												class="px-2 py-1 text-xs bg-blue-500 hover:bg-blue-600 text-white rounded cursor-pointer"
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
