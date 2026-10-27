<script lang="ts">
	import { t } from '$i18n';
	import type { MigrationProgressEvent } from '$services/migration';

	interface Props {
		events: MigrationProgressEvent[];
		streaming: boolean;
	}

	let { events, streaming }: Props = $props();

	function describe(event: MigrationProgressEvent): string {
		switch (event.kind) {
			case 'started':
				return `${event.space}/${event.label} · ${event.planHash || 'unhashed'}`;
			case 'step_started':
				return `step ${event.stepIdx + 1} started`;
			case 'step_completed':
				return `step ${event.stepIdx + 1} completed · ${event.rows} rows`;
			case 'completed':
				return `${event.stepsCompleted} steps · ${event.rowsMigrated} rows${event.errors.length ? ` · ${event.errors.length} errors` : ''}`;
			case 'failed':
				return event.error || 'failed';
			case 'rolled_back':
				return `${event.stepsCompleted} steps · ${event.rowsMigrated} rows`;
		}
	}

	function tone(event: MigrationProgressEvent): string {
		switch (event.kind) {
			case 'failed':
				return 'bg-error-light text-error';
			case 'completed':
				return event.success
					? 'bg-success-light text-success'
					: 'bg-warning-light text-warning';
			case 'rolled_back':
				return 'bg-alt-light text-alt';
			default:
				return 'bg-info-light text-info';
		}
	}
</script>

<section
	class="ui-card shadow-sm p-6"
>
	<h2 class="text-lg font-semibold text-ink mb-4">
		{t('migration.stream')}
		{#if streaming}
			<span class="ml-2 text-xs font-normal text-muted">…</span>
		{/if}
	</h2>
	{#if events.length === 0}
		<p class="text-xs text-muted">—</p>
	{:else}
		<ul class="space-y-1 max-h-96 overflow-auto">
			{#each events as event, i (i)}
				<li class="flex items-center gap-2 text-xs font-mono">
					<span class="px-1.5 py-0.5 rounded {tone(event)}">{event.kind}</span>
					<span class="text-ink">{describe(event)}</span>
				</li>
			{/each}
		</ul>
	{/if}
</section>
