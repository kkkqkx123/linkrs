<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from '$i18n';
	import { connectionStore } from '$stores/connection';
	import { connectionService } from '$services/connection';
	import type { HealthResponse } from '$services/connection';
	import {
		statisticsService,
		type SystemResourceResponse,
	} from '$services/statistics';
	import { formatBytes } from '$utils/metricsFormat';

	let isConnected = $state(false);
	let isChecking = $state(false);
	let panelOpen = $state(false);
	let healthData = $state<HealthResponse | null>(null);
	let systemData = $state<SystemResourceResponse | null>(null);
	let lastCheckTime = $state<string | null>(null);
	let latency = $state<number | null>(null);
	let checkError = $state<string | null>(null);
	let panelEl = $state<HTMLDivElement>();

	onMount(() => {
		const unsub = connectionStore.subscribe((s) => {
			isConnected = s.isConnected && s.isVerified;
		});
		return unsub;
	});

	function handleClickOutside(e: MouseEvent) {
		if (panelEl && !panelEl.contains(e.target as Node)) {
			panelOpen = false;
		}
	}

	$effect(() => {
		if (panelOpen) {
			document.addEventListener('click', handleClickOutside);
		}
		return () => {
			document.removeEventListener('click', handleClickOutside);
		};
	});

	async function handleCheckHealth() {
		isChecking = true;
		checkError = null;
		const start = performance.now();
		try {
			const result = await connectionService.health();
			latency = Math.round(performance.now() - start);
			healthData = result;
			lastCheckTime = new Date().toLocaleTimeString();
			if (result.status !== 'healthy') {
				checkError = t('errors.healthCheckFailed', { status: result.status ?? 'unknown' });
			}
		} catch (err) {
			latency = Math.round(performance.now() - start);
			checkError =
				err instanceof Error
					? err.message
					: t('notification.healthCheckFailed');
			healthData = null;
		}
		// System detail is best-effort: keep the existing rows on failure.
		try {
			systemData = await statisticsService.system();
		} catch {
			/* keep previous system snapshot */
		} finally {
			isChecking = false;
		}
	}

	function togglePanel() {
		panelOpen = !panelOpen;
		if (panelOpen && !healthData) {
			handleCheckHealth();
		}
	}
</script>

<div class="relative" bind:this={panelEl}>
	<button
		class="flex items-center gap-1.5 px-2 py-1 rounded hover:bg-hover transition-colors cursor-pointer text-sm"
		onclick={togglePanel}
		title={t('common.healthCheck')}
	>
		<span
			class="inline-block w-2 h-2 rounded-full {isChecking
				? 'bg-warning animate-pulse'
				: isConnected
					? 'bg-success'
					: 'bg-error'}"
		></span>
		<span class="hidden sm:inline text-muted">
			{isChecking
				? t('common.loading')
				: isConnected
					? t('common.connected')
					: t('common.disconnected')}
		</span>
	</button>

	{#if panelOpen}
		<div
			class="absolute right-0 top-full mt-2 w-72 ui-card ui-card-flush shadow-lg z-50 animate-fade-in"
		>
			<div class="p-4">
				<div class="flex items-center justify-between mb-3">
					<h3 class="font-semibold text-ink text-sm">
						{t('common.healthCheck')}
					</h3>
					<button
						class="ui-btn ui-btn-primary ui-btn-sm"
						onclick={handleCheckHealth}
						disabled={isChecking}
					>
						{isChecking ? t('common.loading') : t('common.refresh')}
					</button>
				</div>

				<div class="space-y-2 text-sm">
					<div class="flex justify-between items-center">
						<span class="text-muted"
							>{t('common.status')}</span
						>
						<span class="flex items-center gap-1.5">
							<span
								class="inline-block w-1.5 h-1.5 rounded-full {isConnected
									? 'bg-success'
									: 'bg-error'}"
							></span>
							<span
								class="{isConnected
									? 'text-success'
									: 'text-error'} font-medium"
							>
								{isConnected ? t('common.connected') : t('common.disconnected')}
							</span>
						</span>
					</div>

					{#if healthData}
						<div class="flex justify-between">
							<span class="text-muted"
								>{t('common.service')}</span
							>
							<span class="text-ink font-mono"
								>{healthData.service}</span
							>
						</div>
						<div class="flex justify-between">
							<span class="text-muted"
								>{t('common.version')}</span
							>
							<span class="text-ink font-mono"
								>{healthData.version}</span
							>
						</div>
						<div class="flex justify-between">
							<span class="text-muted"
								>{t('common.healthStatus')}</span
							>
							<span
								class="font-medium {healthData.status === 'healthy'
									? 'text-success'
									: 'text-warning'}"
							>
								{healthData.status === 'healthy'
									? t('common.healthy')
									: t('common.unhealthy')}
							</span>
						</div>
					{/if}

					{#if latency !== null}
						<div class="flex justify-between">
							<span class="text-muted"
								>{t('common.latency')}</span
							>
							<span
								class="text-ink font-mono {latency >
								1000
									? 'text-warning'
									: latency > 500
										? 'text-warning'
										: ''}"
							>
								{latency}ms
							</span>
						</div>
					{/if}

					{#if systemData}
						<div class="flex justify-between">
							<span class="text-muted"
								>{t('common.processMemory')}</span
							>
							<span class="text-ink font-mono">
								{systemData.process_memory_bytes != null
									? formatBytes(systemData.process_memory_bytes)
									: '—'}
							</span>
						</div>
						<div class="flex justify-between">
							<span class="text-muted"
								>{t('common.diskUsage')}</span
							>
							<span class="text-ink font-mono">
								{systemData.data_dir_size_bytes != null
									? formatBytes(systemData.data_dir_size_bytes)
									: '—'}
							</span>
						</div>
						<div class="flex justify-between">
							<span class="text-muted"
								>{t('common.activeConnections')}</span
							>
							<span class="text-ink font-mono">
								{systemData.connections?.active ?? '—'}
							</span>
						</div>
					{/if}

					{#if lastCheckTime}
						<div class="flex justify-between">
							<span class="text-muted"
								>{t('common.lastCheck')}</span
							>
							<span class="text-muted text-xs"
								>{lastCheckTime}</span
							>
						</div>
					{/if}

					{#if checkError}
						<div
							class="ui-alert ui-alert-error mt-2"
						>
							{checkError}
						</div>
					{/if}
				</div>
			</div>
		</div>
	{/if}
</div>
