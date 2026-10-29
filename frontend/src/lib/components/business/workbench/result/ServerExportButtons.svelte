<script lang="ts">
	import { t } from '$i18n';
	import IconButton from '$components/common/IconButton.svelte';
	import { exportStreamViaServer } from '$utils/export';

	// Shared server-side export buttons (CSV / JSONL). The statement is
	// re-executed server-side; failures surface as an inline dismissible alert.
	let { query }: { query: string } = $props();

	let exportError = $state<string | null>(null);

	async function run(format: 'csv' | 'jsonl') {
		exportError = null;
		try {
			await exportStreamViaServer(query, format);
		} catch (error) {
			exportError =
				error instanceof Error
					? error.message
					: t('notification.serverExportFailed');
		}
	}
</script>

{#if exportError}
	<div class="ui-alert ui-alert-error mb-3 flex items-start gap-2">
		<p class="flex-1 text-error text-sm">{exportError}</p>
		<IconButton
			label={t('common.dismiss')}
			class="ui-link-danger text-sm"
			onclick={() => {
				exportError = null;
			}}
			size={14}
		/>
	</div>
{/if}
<div class="h-4 w-px bg-line"></div>
<button
	class="ui-link text-xs"
	title={t('console.serverExportHint')}
	onclick={() => void run('csv')}>CSV</button
>
<button
	class="ui-link text-xs"
	title={t('console.serverExportHint')}
	onclick={() => void run('jsonl')}>JSONL</button
>
