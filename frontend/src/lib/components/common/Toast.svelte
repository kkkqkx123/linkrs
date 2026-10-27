<script lang="ts">
	import { fromStore } from 'svelte/store';
	import { t } from '$i18n';
	import {
		notificationStore,
		type NotificationType,
	} from '$stores/notification';

	const notifState = fromStore(notificationStore);

	const iconMap: Record<NotificationType, string> = {
		success: '✓',
		error: '✕',
		warning: '⚠',
		info: 'ℹ',
	};

	const colorMap: Record<
		NotificationType,
		{ bg: string; border: string; icon: string; text: string }
	> = {
		success: {
			bg: 'bg-success-light',
			border: 'border-success/40',
			icon: 'text-success',
			text: 'text-success',
		},
		error: {
			bg: 'bg-error-light',
			border: 'border-error/40',
			icon: 'text-error',
			text: 'text-error',
		},
		warning: {
			bg: 'bg-warning-light',
			border: 'border-warning/40',
			icon: 'text-warning',
			text: 'text-warning',
		},
		info: {
			bg: 'bg-info-light',
			border: 'border-info/40',
			icon: 'text-info',
			text: 'text-info',
		},
	};
</script>

{#if notifState.current.length > 0}
	<div
		class="fixed top-4 right-4 z-[9999] flex flex-col gap-2 max-w-sm w-full pointer-events-none"
	>
		{#each notifState.current as notif (notif.id)}
			{@const colors = colorMap[notif.type]}
			<div
				class="pointer-events-auto {colors.bg} {colors.border} border rounded-lg shadow-lg p-4 flex items-start gap-3 animate-slide-in-right"
				role="alert"
			>
				<span
					class="flex-shrink-0 w-5 h-5 flex items-center justify-center rounded-full text-xs font-bold {colors.icon}"
				>
					{iconMap[notif.type]}
				</span>
				<div class="flex-1 min-w-0">
					<p class="text-sm font-medium {colors.text}">
						{t(notif.key, notif.values)}
					</p>
					{#if notif.description}
						<p class="text-xs mt-0.5 opacity-80 {colors.text}">
							{notif.description}
						</p>
					{/if}
				</div>
				<button
					class="flex-shrink-0 {colors.text} opacity-60 hover:opacity-100 transition-opacity cursor-pointer"
					aria-label={t('common.dismiss')}
					onclick={() => notificationStore.dismiss(notif.id)}
				>
					✕
				</button>
			</div>
		{/each}
	</div>
{/if}
