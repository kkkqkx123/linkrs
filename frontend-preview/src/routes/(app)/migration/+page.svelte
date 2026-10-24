<script lang="ts">
	import { t } from '$i18n';
	import MigrationPlanForm from '$components/business/MigrationPlanForm.svelte';
	import MigrationExecutePanel from '$components/business/MigrationExecutePanel.svelte';
	import MigrationProgressList from '$components/business/MigrationProgressList.svelte';
	import { migrationService, type MigrationProgressEvent } from '$services/migration';

	let space = $state('');
	let label = $state('');
	let fromVersion = $state(1);
	let toVersion = $state(2);
	let isEdge = $state(false);
	let expandContract = $state(false);
	let planJson = $state('');
	let progressEvents = $state<MigrationProgressEvent[]>([]);
	let streaming = $state(false);
	let streamController: AbortController | null = null;

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
		streamController?.abort();
		streamController = new AbortController();
		streaming = true;
		error = null;
		progressEvents = [];
		try {
			const outcome = await migrationService.streamProgress(space, label, {
				isEdge,
				signal: streamController.signal,
				onEvent: (event) => {
					progressEvents = [...progressEvents, event];
				},
			});
			if (outcome.streamError) error = outcome.streamError;
		} catch (err) {
			error = err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			streaming = false;
		}
	}

	function stopStream() {
		streamController?.abort();
	}
</script>

<div class="max-w-6xl mx-auto space-y-4 animate-fade-in pb-8">
	<h1 class="text-xl font-bold text-gray-800 dark:text-gray-100">
		{t('sidebar.migration')}
	</h1>

	{#if error}
		<p class="text-xs text-red-500">{error}</p>
	{/if}

	<MigrationPlanForm
		bind:space
		bind:label
		bind:fromVersion
		bind:toVersion
		bind:isEdge
		bind:expandContract
		{loading}
		{streaming}
		onCreatePlan={() =>
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
		onStatus={() => run(() => migrationService.status(space, label, isEdge))}
		onHistory={() => run(() => migrationService.history(space, label, isEdge))}
		onStream={streamProgress}
		onStopStream={stopStream}
	/>

	<MigrationExecutePanel
		bind:planJson
		{loading}
		onDryRun={() => run(() => migrationService.dryRun(planJson), true)}
		onExecute={() => run(() => migrationService.execute(planJson), true)}
		onRollback={() => run(() => migrationService.rollback(planJson), true)}
	/>

	{#if resultText}
		<section
			class="bg-white dark:bg-[#1C2333] rounded-xl p-5 border border-gray-100 dark:border-gray-700/50 shadow-sm"
		>
			<pre
				class="text-xs font-mono bg-gray-50 dark:bg-gray-800/50 p-3 rounded border border-gray-200 dark:border-gray-700 overflow-auto max-h-96 text-gray-700 dark:text-gray-300">{resultText}</pre
			>
		</section>
	{/if}

	<MigrationProgressList events={progressEvents} {streaming} />
</div>
