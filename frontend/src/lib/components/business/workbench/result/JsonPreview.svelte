<script lang="ts">
	import { t } from '$i18n';
	import { STREAM_JSON_PREVIEW_LIMIT } from '$utils/virtualWindow';

	// Shared JSON preview block with the large-payload truncation notice.
	let {
		columns,
		rows,
		rowCount,
	}: {
		columns: string[];
		rows: Record<string, unknown>[];
		rowCount: number;
	} = $props();

	let json = $derived.by(() => {
		const shown = rows.slice(0, STREAM_JSON_PREVIEW_LIMIT);
		return JSON.stringify({ columns, rows: shown, rowCount }, null, 2);
	});

	let truncated = $derived(rows.length > STREAM_JSON_PREVIEW_LIMIT);
</script>

{#if truncated}
	<p class="mb-2 text-xs text-muted">
		{t('console.streamJsonTruncated', {
			shown: STREAM_JSON_PREVIEW_LIMIT,
			total: rowCount,
		})}
	</p>
{/if}
<pre class="ui-pre ui-pre-lg">{json}</pre>
