<script lang="ts">
	import { t } from '$i18n';
	import { goto } from '$app/navigation';
	import { consoleStore } from '$stores/console';
	import { statisticsService, type QueryProfileDetailResponse } from '$services/statistics';
	import { formatCount, formatLatencyMs } from '$utils/metricsFormat';
	import { copyToClipboard } from '$utils/function';
	import Drawer from '$components/common/Drawer.svelte';

	let { traceId, onclose }: { traceId: string; onclose: () => void } = $props();

	let portrait = $state<QueryProfileDetailResponse | null>(null);
	let portraitLoading = $state(false);
	let portraitError = $state<string | null>(null);
	let copied = $state(false);

	$effect(() => {
		const id = traceId;
		if (!id) return;
		portrait = null;
		portraitError = null;
		portraitLoading = true;
		copied = false;
		statisticsService
			.queryProfile(id)
			.then((result) => {
				portrait = result;
			})
			.catch((err: unknown) => {
				portraitError =
					err instanceof Error ? err.message : t('notification.requestFailed');
			})
			.finally(() => {
				portraitLoading = false;
			});
	});

	async function copyTrace() {
		copied = await copyToClipboard(traceId);
	}

	function backToConsole() {
		if (portrait?.query) consoleStore.loadFromHistory(portrait.query);
		onclose();
		goto('/console');
	}
</script>

<Drawer title={t('monitoring.portrait')} onClose={onclose} class="w-full max-w-md">
	{#if portraitLoading}
		<p class="text-sm text-muted">{t('monitoring.loading')}</p>
	{:else if portraitError}
		<p class="text-sm text-error">{portraitError}</p>
	{:else if portrait}
		<div class="space-y-3 text-sm">
			<div class="font-mono text-xs break-all bg-secondary rounded p-2">
				{portrait.trace_id}
			</div>
			<div class="flex gap-2">
				<button class="ui-btn ui-btn-soft ui-btn-xs" onclick={copyTrace}>
					{copied ? t('monitoring.copied') : t('monitoring.copyTrace')}
				</button>
				<button class="ui-btn ui-btn-info ui-btn-xs" onclick={backToConsole}>
					{t('monitoring.backToConsole')}
				</button>
			</div>
			<div class="font-mono text-xs break-words bg-secondary rounded p-2">
				{portrait.query}
			</div>
			<div class="grid grid-cols-2 gap-2 font-mono">
				<div>
					<span class="text-muted">{t('monitoring.duration')}: </span
					>{formatLatencyMs(portrait.duration_ms)}
				</div>
				<div>
					<span class="text-muted">status: </span>{portrait.status}
				</div>
				<div>
					<span class="text-muted">{t('monitoring.resultCount')}: </span
					>{formatCount(portrait.result_count)}
				</div>
				<div>
					<span class="text-muted">{t('monitoring.planNodes')}: </span
					>{formatCount(portrait.plan_node_count)}
				</div>
			</div>
			{#if portrait.stages}
				<div>
					<p class="text-xs font-medium text-muted mb-1">
						{t('monitoring.stages')}
					</p>
					<ul class="font-mono space-y-1">
						<li class="flex justify-between">
							<span>{t('monitoring.stageParse')}</span><span
								>{formatLatencyMs(portrait.stages.parse_ms)}</span
							>
						</li>
						<li class="flex justify-between">
							<span>{t('monitoring.stageValidate')}</span><span
								>{formatLatencyMs(portrait.stages.validate_ms)}</span
							>
						</li>
						<li class="flex justify-between">
							<span>{t('monitoring.stagePlan')}</span><span
								>{formatLatencyMs(portrait.stages.plan_ms)}</span
							>
						</li>
						<li class="flex justify-between">
							<span>{t('monitoring.stageOptimize')}</span><span
								>{formatLatencyMs(portrait.stages.optimize_ms)}</span
							>
						</li>
						<li class="flex justify-between">
							<span>{t('monitoring.stageExecute')}</span><span
								>{formatLatencyMs(portrait.stages.execute_ms)}</span
							>
						</li>
					</ul>
				</div>
			{/if}
			{#if portrait.executors && portrait.executors.length > 0}
				<div>
					<p class="text-xs font-medium text-muted mb-1">
						{t('monitoring.stageExecutors')}
					</p>
					<ul class="font-mono space-y-1">
						{#each portrait.executors as ex (ex.executor_type)}
							<li class="flex justify-between">
								<span>{ex.executor_type}</span>
								<span
									>{formatLatencyMs(ex.duration_ms)} / {formatCount(
										ex.rows,
									)}</span
								>
							</li>
						{/each}
					</ul>
				</div>
			{/if}
			{#if portrait.error}
				<div>
					<p class="text-xs font-medium text-muted mb-1">
						{t('monitoring.errorDetail')}
					</p>
					<p
						class="font-mono text-xs text-error break-words bg-error-light rounded p-2"
					>
						{portrait.error}
					</p>
				</div>
			{/if}
		</div>
	{/if}
</Drawer>
