<script lang="ts">
	import { t } from '$i18n';
	import { migrationService } from '$services/migration';

	let space = $state('');
	let label = $state('');
	let fromVersion = $state(1);
	let toVersion = $state(2);
	let isEdge = $state(false);
	let expandContract = $state(false);
	let planJson = $state('');
	let streamText = $state('');
	let streaming = $state(false);

	let loading = $state(false);
	let error = $state<string | null>(null);
	let resultText = $state('');

	function stringify(payload: unknown): string {
		try {
			return JSON.stringify(payload, null, 2);
		} catch {
			return String(payload ?? '');
		}
	}

	async function run(fn: () => Promise<unknown>, keepPlan = false) {
		loading = true;
		error = null;
		try {
			const payload = await fn();
			resultText = stringify(payload);
			if (!keepPlan) {
				const plan = (payload as Record<string, unknown> | null)?.plan_json;
				if (typeof plan === 'string' && plan) planJson = plan;
			}
		} catch (err) {
			error = err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			loading = false;
		}
	}

	async function streamProgress() {
		streaming = true;
		error = null;
		streamText = '';
		try {
			await migrationService.streamProgress(
				space,
				label,
				isEdge,
				(chunk) => {
					streamText += chunk;
				},
			);
		} catch (err) {
			error = err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			streaming = false;
		}
	}
</script>

<div class="max-w-6xl mx-auto space-y-4 animate-fade-in pb-8">
	<h1 class="text-xl font-bold text-gray-800 dark:text-gray-100">
		{t('sidebar.migration')}
	</h1>

	{#if error}
		<p class="text-xs text-red-500">{error}</p>
	{/if}

	<section
		class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
	>
		<h2 class="font-semibold text-gray-800 dark:text-gray-100 mb-3">
			{t('migration.plan')}
		</h2>
		<div class="grid md:grid-cols-4 gap-2 text-sm mb-3">
			<label class="flex flex-col gap-1 text-xs text-gray-500">
				{t('migration.space')}
				<input
					class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
					bind:value={space}
				/>
			</label>
			<label class="flex flex-col gap-1 text-xs text-gray-500">
				{t('migration.label')}
				<input
					class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
					bind:value={label}
				/>
			</label>
			<label class="flex flex-col gap-1 text-xs text-gray-500">
				{t('migration.fromVersion')}
				<input
					type="number"
					class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
					bind:value={fromVersion}
				/>
			</label>
			<label class="flex flex-col gap-1 text-xs text-gray-500">
				{t('migration.toVersion')}
				<input
					type="number"
					class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
					bind:value={toVersion}
				/>
			</label>
		</div>
		<div class="flex flex-wrap items-center gap-4 text-xs text-gray-500 mb-3">
			<label class="flex items-center gap-1">
				<input type="checkbox" bind:checked={isEdge} />
				{t('migration.isEdge')}
			</label>
			<label class="flex items-center gap-1">
				<input type="checkbox" bind:checked={expandContract} />
				{t('migration.expandContract')}
			</label>
		</div>
		<div class="flex flex-wrap gap-2">
			<button
				class="px-3 py-1 text-xs rounded bg-blue-500 hover:bg-blue-600 text-white disabled:opacity-50 cursor-pointer"
				onclick={() =>
					run(() =>
						migrationService.plan({
							space,
							label,
							from_version: fromVersion,
							to_version: toVersion,
							is_edge: isEdge,
							expand_contract: expandContract,
						}),
					)}
				disabled={loading}
			>
				{t('migration.createPlan')}
			</button>
			<button
				class="px-3 py-1 text-xs rounded bg-gray-100 dark:bg-gray-700/50 text-gray-700 dark:text-gray-200 disabled:opacity-50 cursor-pointer"
				onclick={() => run(() => migrationService.status(space, label, isEdge))}
				disabled={loading}
			>
				{t('common.status')}
			</button>
			<button
				class="px-3 py-1 text-xs rounded bg-gray-100 dark:bg-gray-700/50 text-gray-700 dark:text-gray-200 disabled:opacity-50 cursor-pointer"
				onclick={() => run(() => migrationService.history(space, label, isEdge))}
				disabled={loading}
			>
				{t('migration.history')}
			</button>
			<button
				class="px-3 py-1 text-xs rounded bg-gray-100 dark:bg-gray-700/50 text-gray-700 dark:text-gray-200 disabled:opacity-50 cursor-pointer"
				onclick={streamProgress}
				disabled={streaming}
			>
				{t('migration.stream')}
			</button>
		</div>
	</section>

	<section
		class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
	>
		<h2 class="font-semibold text-gray-800 dark:text-gray-100 mb-3">
			{t('migration.execute')}
		</h2>
		<label class="flex flex-col gap-1 text-xs text-gray-500 mb-3">
			{t('migration.planJson')}
			<textarea
				rows="4"
				class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
				bind:value={planJson}
			></textarea>
		</label>
		<div class="flex flex-wrap gap-2">
			<button
				class="px-3 py-1 text-xs rounded bg-amber-500 hover:bg-amber-600 text-white disabled:opacity-50 cursor-pointer"
				onclick={() => run(() => migrationService.dryRun(planJson), true)}
				disabled={loading || !planJson}
			>
				{t('migration.dryRun')}
			</button>
			<button
				class="px-3 py-1 text-xs rounded bg-blue-500 hover:bg-blue-600 text-white disabled:opacity-50 cursor-pointer"
				onclick={() => run(() => migrationService.execute(planJson), true)}
				disabled={loading || !planJson}
			>
				{t('migration.run')}
			</button>
			<button
				class="px-3 py-1 text-xs rounded bg-red-500 hover:bg-red-600 text-white disabled:opacity-50 cursor-pointer"
				onclick={() => run(() => migrationService.rollback(planJson), true)}
				disabled={loading || !planJson}
			>
				{t('migration.rollback')}
			</button>
		</div>
	</section>

	{#if resultText}
		<section
			class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
		>
			<pre
				class="text-xs font-mono bg-gray-50 dark:bg-gray-800/50 p-3 rounded border border-gray-200 dark:border-gray-700 overflow-auto max-h-96 text-gray-700 dark:text-gray-300">{resultText}</pre
			>
		</section>
	{/if}

	{#if streamText}
		<section
			class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
		>
			<pre
				class="text-xs font-mono bg-gray-50 dark:bg-gray-800/50 p-3 rounded border border-gray-200 dark:border-gray-700 overflow-auto max-h-96 text-gray-700 dark:text-gray-300">{streamText}</pre
			>
		</section>
	{/if}
</div>
