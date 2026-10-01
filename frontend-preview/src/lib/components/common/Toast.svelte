<script lang="ts">
  import { notificationStore, type Notification, type NotificationType } from '$stores/notification';

  let notifications = $state<Notification[]>([]);
  notificationStore.subscribe(n => notifications = n);

  const iconMap: Record<NotificationType, string> = {
    success: '✓',
    error: '✕',
    warning: '⚠',
    info: 'ℹ',
  };

  const colorMap: Record<NotificationType, { bg: string; border: string; icon: string; text: string }> = {
    success: { bg: 'bg-green-50 dark:bg-green-900/20', border: 'border-green-200 dark:border-green-800', icon: 'text-green-500 dark:text-green-400', text: 'text-green-800 dark:text-green-200' },
    error: { bg: 'bg-red-50 dark:bg-red-900/20', border: 'border-red-200 dark:border-red-800', icon: 'text-red-500 dark:text-red-400', text: 'text-red-800 dark:text-red-200' },
    warning: { bg: 'bg-yellow-50 dark:bg-yellow-900/20', border: 'border-yellow-200 dark:border-yellow-800', icon: 'text-yellow-500 dark:text-yellow-400', text: 'text-yellow-800 dark:text-yellow-200' },
    info: { bg: 'bg-blue-50 dark:bg-blue-900/20', border: 'border-blue-200 dark:border-blue-800', icon: 'text-blue-500 dark:text-blue-400', text: 'text-blue-800 dark:text-blue-200' },
  };
</script>

{#if notifications.length > 0}
  <div class="fixed top-4 right-4 z-[9999] flex flex-col gap-2 max-w-sm w-full pointer-events-none">
    {#each notifications as notif (notif.id)}
      {@const colors = colorMap[notif.type]}
      <div
        class="pointer-events-auto {colors.bg} {colors.border} border rounded-lg shadow-lg p-4 flex items-start gap-3 animate-slide-in-right"
        role="alert"
      >
        <span class="flex-shrink-0 w-5 h-5 flex items-center justify-center rounded-full text-xs font-bold {colors.icon}">
          {iconMap[notif.type]}
        </span>
        <div class="flex-1 min-w-0">
          <p class="text-sm font-medium {colors.text}">{notif.message}</p>
          {#if notif.description}
            <p class="text-xs mt-0.5 opacity-80 {colors.text}">{notif.description}</p>
          {/if}
        </div>
        <button
          class="flex-shrink-0 {colors.text} opacity-60 hover:opacity-100 transition-opacity cursor-pointer"
          onclick={() => notificationStore.dismiss(notif.id)}
        >
          ✕
        </button>
      </div>
    {/each}
  </div>
{/if}

<style>
  @keyframes slide-in-right {
    from { transform: translateX(100%); opacity: 0; }
    to { transform: translateX(0); opacity: 1; }
  }
  .animate-slide-in-right {
    animation: slide-in-right 0.3s ease-out;
  }
</style>