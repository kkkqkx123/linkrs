import { writable } from 'svelte/store';
import type { MessageKey } from '$i18n';

export type NotificationType = 'success' | 'error' | 'warning' | 'info';

export interface Notification {
	id: string;
	type: NotificationType;
	/** Message key; the text is rendered when the toast is displayed. */
	key: MessageKey;
	values?: Record<string, string | number>;
	description?: string;
	duration?: number;
}

function createNotificationStore() {
	const { subscribe, update } = writable<Notification[]>([]);

	let counter = 0;

	function add(n: Omit<Notification, 'id'>) {
		const id = `notif-${Date.now()}-${counter++}`;
		const notif: Notification = { ...n, id };
		update((list) => [...list, notif]);
		const duration = n.duration ?? 4000;
		if (duration > 0) {
			setTimeout(() => {
				update((list) => list.filter((item) => item.id !== id));
			}, duration);
		}
		return id;
	}

	const show =
		(type: NotificationType) =>
		(
			key: MessageKey,
			values?: Record<string, string | number>,
			description?: string,
		) =>
			add({ type, key, values, description });

	return {
		subscribe,
		success: show('success'),
		error: show('error'),
		warning: show('warning'),
		info: show('info'),
		dismiss: (id: string) => update((list) => list.filter((n) => n.id !== id)),
		clear: () => update(() => []),
	};
}

export const notificationStore = createNotificationStore();
