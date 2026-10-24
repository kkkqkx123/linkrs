<script lang="ts">
	import { t } from '$i18n';
	import { fulltextService } from '$services/fulltext';

	let spaceId = $state(1);
	let tagName = $state('');
	let fieldName = $state('');
	let rebuildId = $state('');

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

	async function run(fn: () => Promise<unknown>) {
		loading = true;
		error = null;
		try {
			resultText = stringify(await fn());
		} catch (err) {
			error = err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			loading = false;
		}
	}
</script>

<div class="max-w-6xl mx-auto space-y-4 animate-fade-in pb-8">
	<div class="flex items-center justify-between flex-wrap gap-2">
		<h1 class="text-xl font-bold text-gray-800 dark:text-gray-100">
			{t('sidebar.fulltext')}
		</h1>
		<button
			class="px-3 py-1 text-sm rounded bg-blue-500 hover:bg-blue-600 text-white disabled:opacity-50 cursor-pointer"
			onclick={() => run(() => fulltextService.inconsistent())}
			disabled={loading}
		>
			{loading ? t('common.loading') : t('fulltext.inconsistent')}
		</button>
	</div>

	{#if error}
		<p class="text-xs text-red-500">{error}</p>
	{/if}

	<section
		class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
	>
		<h2 class="font-semibold text-gray-800 dark:text-gray-100 mb-3">
			{t('fulltext.manage')}
		</h2>
		<div class="grid md:grid-cols-3 gap-2 text-sm mb-3">
			<label class="flex flex-col gap-1 text-xs text-gray-500">
				{t('vector.spaceId')}
				<input
					type="number"
					class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
					bind:value={spaceId}
				/>
			</label>
			<label class="flex flex-col gap-1 text-xs text-gray-500">
				{t('vector.tag')}
				<input
					class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
					bind:value={tagName}
				/>
			</label>
			<label class="flex flex-col gap-1 text-xs text-gray-500">
				{t('vector.field')}
				<input
					class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
					bind:value={fieldName}
				/>
			</label>
		</div>
		<label class="flex flex-col gap-1 text-xs text-gray-500 mb-3">
			{t('vector.rebuildId')}
			<input
				class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
				bind:value={rebuildId}
			/>
		</label>
		<div class="flex flex-wrap gap-2">
			<button
				class="px-3 py-1 text-xs rounded bg-amber-500 hover:bg-amber-600 text-white disabled:opacity-50 cursor-pointer"
				onclick={() =>
					run(() =>
						fulltextService.rebuild({
							space_id: spaceId,
							tag_name: tagName,
							field_name: fieldName,
						}),
					)}
				disabled={loading}
			>
				{t('schema.rebuild')}
			</button>
			<button
				class="px-3 py-1 text-xs rounded bg-gray-500 hover:bg-gray-600 text-white disabled:opacity-50 cursor-pointer"
				onclick={() =>
					run(() =>
						fulltextService.clear({
							space_id: spaceId,
							tag_name: tagName,
							field_name: fieldName,
						}),
					)}
				disabled={loading}
			>
				{t('common.clear')}
			</button>
			<button
				class="px-3 py-1 text-xs rounded bg-gray-100 dark:bg-gray-700/50 text-gray-700 dark:text-gray-200 disabled:opacity-50 cursor-pointer"
				onclick={() => run(() => fulltextService.rebuildStatus(rebuildId))}
				disabled={loading || !rebuildId}
			>
				{t('vector.rebuildStatus')}
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
</div>
