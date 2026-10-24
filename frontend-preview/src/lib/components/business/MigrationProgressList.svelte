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
				return 'bg-red-100 text-red-700 dark:bg-red-900/40 dark:text-red-300';
			case 'completed':
				return event.success
					? 'bg-green-100 text-green-700 dark:bg-green-900/40 dark:text-green-300'
					: 'bg-amber-100 text-amber-700 dark:bg-amber-900/40 dark:text-amber-300';
			case 'rolled_back':
				return 'bg-purple-100 text-purple-700 dark:bg-purple-900/40 dark:text-purple-300';
			default:
				return 'bg-blue-100 text-blue-700 dark:bg-blue-900/40 dark:text-blue-300';
		}
	}
</script>

<section
	class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
>
	<h2 class="font-semibold text-gray-800 dark:text-gray-100 mb-3">
		{t('migration.stream')}
		{#if streaming}
			<span class="ml-2 text-xs font-normal text-gray-400">…</span>
		{/if}
	</h2>
	{#if events.length === 0}
		<p class="text-xs text-gray-400">—</p>
	{:else}
		<ul class="space-y-1 max-h-96 overflow-auto">
			{#each events as event, i (i)}
				<li class="flex items-center gap-2 text-xs font-mono">
					<span class="px-1.5 py-0.5 rounded {tone(event)}">{event.kind}</span>
					<span class="text-gray-700 dark:text-gray-300">{describe(event)}</span>
				</li>
			{/each}
		</ul>
	{/if}
</section>
