import { writable } from 'svelte/store';

export type NotificationType = 'success' | 'error' | 'warning' | 'info';

export interface Notification {
  id: string;
  type: NotificationType;
  message: string;
  description?: string;
  duration?: number;
}

function createNotificationStore() {
  const { subscribe, update } = writable<Notification[]>([]);

  let counter = 0;

  function add(n: Omit<Notification, 'id'>) {
    const id = `notif-${Date.now()}-${counter++}`;
    const notif: Notification = { ...n, id };
    update(list => [...list, notif]);
    const duration = n.duration ?? 4000;
    if (duration > 0) {
      setTimeout(() => {
        update(list => list.filter(item => item.id !== id));
      }, duration);
    }
    return id;
  }

  return {
    subscribe,
    success: (message: string, description?: string) => add({ type: 'success', message, description }),
    error: (message: string, description?: string) => add({ type: 'error', message, description }),
    warning: (message: string, description?: string) => add({ type: 'warning', message, description }),
    info: (message: string, description?: string) => add({ type: 'info', message, description }),
    dismiss: (id: string) => update(list => list.filter(n => n.id !== id)),
    clear: () => update(() => []),
  };
}

export const notificationStore = createNotificationStore();