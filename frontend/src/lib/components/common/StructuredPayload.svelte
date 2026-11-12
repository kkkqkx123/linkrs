<script lang="ts">
	import { t } from '$i18n';
	import EmptyState from '$components/common/EmptyState.svelte';
	import { Copy } from '@lucide/svelte';
	import {
		classifyPayload,
		recordColumns,
	} from '$utils/structuredPayload';
	import { copyToClipboard } from '$utils/function';

	let { payload }: { payload: unknown } = $props();

	const classified = $derived(classifyPayload(payload));
	const columns = $derived(
		classified.shape === 'records' ? recordColumns(classified.records) : [],
	);

	const jsonText = $derived.by(() => {
		try {
			return JSON.stringify(payload, null, 2);
		} catch {
			return String(payload ?? '');
		}
	});

	/** Cell text: objects compact to one-line JSON, missing values empty. */
	function formatCell(value: unknown): string {
		if (value === null || value === undefined) return '';
		if (typeof value === 'object') {
			try {
				return JSON.stringify(value);
			} catch {
				return String(value);
			}
		}
		return String(value);
	}

	async function copyJson() {
		await copyToClipboard(jsonText);
	}
</script>

{#if classified.shape === 'records'}
	<div class="overflow-x-auto">
		<table class="ui-table w-full border-collapse">
			<thead>
				<tr class="bg-secondary">
					{#each columns as col (col)}
						<th>{col}</th>
					{/each}
				</tr>
			</thead>
			<tbody>
				{#each classified.records as row, i (i)}
					<tr>
						{#each columns as col (col)}
							<td class="max-w-60 truncate font-mono text-xs">
								{formatCell(row[col])}
							</td>
						{/each}
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
{:else if classified.shape === 'strings'}
	{#if classified.values.length === 0}
		<EmptyState message={t('dataBrowser.retrieval.noResults')} />
	{:else}
		<div class="flex flex-wrap gap-2">
			{#each classified.values as value, i (i)}
				<span class="ui-chip font-mono">{value}</span>
			{/each}
		</div>
	{/if}
{:else if classified.shape === 'primitive'}
	<div class="flex items-center gap-2">
		<p class="font-mono text-sm break-all">{classified.values[0]}</p>
		<button
			type="button"
			class="ui-icon-btn flex-shrink-0"
			aria-label={t('common.copy')}
			onclick={copyJson}
		>
			<Copy size={16} />
		</button>
	</div>
{:else}
	<div class="relative">
		<button
			type="button"
			class="ui-icon-btn flex-shrink-0 absolute top-1 right-1"
			aria-label={t('common.copy')}
			onclick={copyJson}
		>
			<Copy size={16} />
		</button>
		<pre class="ui-pre mt-3">{jsonText}</pre>
	</div>
{/if}
