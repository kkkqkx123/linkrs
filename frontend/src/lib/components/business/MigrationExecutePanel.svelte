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
	class="ui-card shadow-sm p-5"
>
	<h2 class="font-semibold text-gray-800 dark:text-gray-100 mb-3">
		{t('migration.execute')}
	</h2>
	<label class="flex flex-col gap-1 text-xs text-gray-500 mb-3">
		{t('migration.planJson')}
		<textarea
			rows="4"
			class="ui-input ui-input-sm font-mono"
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
			class="ui-btn ui-btn-primary ui-btn-sm"
			onclick={onExecute}
			disabled={loading || !planJson}
		>
			{t('migration.run')}
		</button>
		<button
			class="ui-btn ui-btn-danger ui-btn-sm"
			onclick={onRollback}
			disabled={loading || !planJson}
		>
			{t('migration.rollback')}
		</button>
	</div>
</section>
