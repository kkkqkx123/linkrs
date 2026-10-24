<script lang="ts">
	import { t } from '$i18n';

	interface Props {
		planJson: string;
		loading: boolean;
		onDryRun: () => void;
		onExecute: () => void;
		onRollback: () => void;
	}

	let {
		planJson = $bindable(),
		loading,
		onDryRun,
		onExecute,
		onRollback,
	}: Props = $props();
</script>

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
			onclick={onDryRun}
			disabled={loading || !planJson}
		>
			{t('migration.dryRun')}
		</button>
		<button
			class="px-3 py-1 text-xs rounded bg-blue-500 hover:bg-blue-600 text-white disabled:opacity-50 cursor-pointer"
			onclick={onExecute}
			disabled={loading || !planJson}
		>
			{t('migration.run')}
		</button>
		<button
			class="px-3 py-1 text-xs rounded bg-red-500 hover:bg-red-600 text-white disabled:opacity-50 cursor-pointer"
			onclick={onRollback}
			disabled={loading || !planJson}
		>
			{t('migration.rollback')}
		</button>
	</div>
</section>
