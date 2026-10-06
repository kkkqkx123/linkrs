<script lang="ts">
	import { t } from '$i18n';
	import { batchService } from '$services/batch';

	let spaceId = $state(1);
	let batchType = $state('mixed');
	let batchSize = $state(1000);
	let batchId = $state('');
	let itemsText = $state('[]');

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

	async function run(fn: () => Promise<unknown>, captureId = false) {
		loading = true;
		error = null;
		try {
			const payload = await fn();
			resultText = stringify(payload);
			if (captureId) {
				const id = (payload as Record<string, unknown> | null)?.batch_id;
				if (typeof id === 'string' && id) batchId = id;
			}
		} catch (err) {
			error = err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			loading = false;
		}
	}

	function parseItems(): unknown[] {
		try {
			const parsed: unknown = JSON.parse(itemsText || '[]');
			return Array.isArray(parsed) ? parsed : [];
		} catch {
			throw new Error(t('batch.invalidItems'));
		}
	}
</script>

<div class="max-w-6xl mx-auto space-y-4 animate-fade-in pb-8">
	<h1 class="text-xl font-bold text-gray-800 dark:text-gray-100">
		{t('sidebar.batch')}
	</h1>

	{#if error}
		<p class="text-xs text-red-500">{error}</p>
	{/if}

	<section
		class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
	>
		<h2 class="font-semibold text-gray-800 dark:text-gray-100 mb-3">
			{t('batch.create')}
		</h2>
		<div class="grid md:grid-cols-3 gap-2 text-sm mb-3">
			<label class="flex flex-col gap-1 text-xs text-gray-500">
				{t('batch.spaceId')}
				<input
					type="number"
					class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
					bind:value={spaceId}
				/>
			</label>
			<label class="flex flex-col gap-1 text-xs text-gray-500">
				{t('batch.type')}
				<select
					class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
					bind:value={batchType}
				>
					<option value="vertex">vertex</option>
					<option value="edge">edge</option>
					<option value="mixed">mixed</option>
				</select>
			</label>
			<label class="flex flex-col gap-1 text-xs text-gray-500">
				{t('batch.size')}
				<input
					type="number"
					class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
					bind:value={batchSize}
				/>
			</label>
		</div>
		<button
			class="px-3 py-1 text-xs rounded bg-blue-500 hover:bg-blue-600 text-white disabled:opacity-50 cursor-pointer"
			onclick={() =>
				run(
					() =>
						batchService.create({
							space_id: spaceId,
							batch_type: batchType,
							batch_size: batchSize,
						}),
					true,
				)}
			disabled={loading}
		>
			{t('common.create')}
		</button>
	</section>

	<section
		class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
	>
		<h2 class="font-semibold text-gray-800 dark:text-gray-100 mb-3">
			{t('batch.manage')}
		</h2>
		<label class="flex flex-col gap-1 text-xs text-gray-500 mb-3">
			{t('batch.id')}
			<input
				class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
				bind:value={batchId}
			/>
		</label>
		<label class="flex flex-col gap-1 text-xs text-gray-500 mb-3">
			{t('batch.items')}
			<textarea
				rows="4"
				class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
				bind:value={itemsText}
			></textarea>
		</label>
		<div class="flex flex-wrap gap-2">
			<button
				class="px-3 py-1 text-xs rounded bg-blue-500 hover:bg-blue-600 text-white disabled:opacity-50 cursor-pointer"
				onclick={() => run(() => batchService.addItems(batchId, parseItems()))}
				disabled={loading || !batchId}
			>
				{t('batch.addItems')}
			</button>
			<button
				class="px-3 py-1 text-xs rounded bg-green-600 hover:bg-green-700 text-white disabled:opacity-50 cursor-pointer"
				onclick={() => run(() => batchService.execute(batchId))}
				disabled={loading || !batchId}
			>
				{t('batch.execute')}
			</button>
			<button
				class="px-3 py-1 text-xs rounded bg-gray-100 dark:bg-gray-700/50 text-gray-700 dark:text-gray-200 disabled:opacity-50 cursor-pointer"
				onclick={() => run(() => batchService.status(batchId))}
				disabled={loading || !batchId}
			>
				{t('common.status')}
			</button>
			<button
				class="px-3 py-1 text-xs rounded bg-amber-500 hover:bg-amber-600 text-white disabled:opacity-50 cursor-pointer"
				onclick={() => run(() => batchService.cancel(batchId))}
				disabled={loading || !batchId}
			>
				{t('common.cancel')}
			</button>
			<button
				class="px-3 py-1 text-xs rounded bg-red-500 hover:bg-red-600 text-white disabled:opacity-50 cursor-pointer"
				onclick={() => run(() => batchService.remove(batchId))}
				disabled={loading || !batchId}
			>
				{t('common.delete')}
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
