import type { LucideIcon } from '@lucide/svelte';
import type { MessageKey } from '$i18n';
import {
	Keyboard,
	Database,
	Table,
	LineChart,
	Route,
	Package,
	Repeat,
	Users,
	PlugZap,
	Activity,
	RefreshCw,
} from '@lucide/svelte';

export interface MenuItem {
	key: string;
	icon: LucideIcon;
	label: MessageKey;
	route?: string;
	/** Only visible to roles allowed to manage users/sessions. */
	requiresUsers?: boolean;
	group: GroupKey;
}

/** Group order drives the rendered section order. */
export const GROUP_ORDER = ['explore', 'data', 'ops'] as const;
export type GroupKey = (typeof GROUP_ORDER)[number];

export const groupLabels: Record<GroupKey, MessageKey> = {
	explore: 'sidebar.group.explore',
	data: 'sidebar.group.data',
	ops: 'sidebar.group.ops',
};

/** Single source of navigation entries, shared by the sidebar and the
 * command palette. */
export const MENU_ITEMS: MenuItem[] = [
	{
		key: '/console',
		icon: Keyboard,
		label: 'sidebar.console',
		route: '/console',
		group: 'explore',
	},
	{
		key: '/data-browser',
		icon: Table,
		label: 'sidebar.dataBrowser',
		route: '/data-browser',
		group: 'explore',
	},
	{
		key: '/schema',
		icon: Database,
		label: 'sidebar.schema',
		route: '/schema',
		group: 'data',
	},
	{
		key: '/batch',
		icon: Package,
		label: 'sidebar.batch',
		route: '/batch',
		group: 'data',
	},
	{
		key: '/transfer',
		icon: Repeat,
		label: 'sidebar.transfer',
		route: '/transfer',
		group: 'data',
	},
	{
		key: '/monitoring',
		icon: LineChart,
		label: 'sidebar.monitoring',
		route: '/monitoring',
		group: 'ops',
	},
	{
		key: '/transactions',
		icon: Activity,
		label: 'sidebar.transactions',
		route: '/transactions',
		group: 'ops',
	},
	{
		key: '/sync',
		icon: RefreshCw,
		label: 'sidebar.sync',
		route: '/sync',
		group: 'ops',
	},
	{
		key: '/migration',
		icon: Route,
		label: 'sidebar.migration',
		route: '/migration',
		group: 'ops',
	},
	{
		key: '/users',
		icon: Users,
		label: 'sidebar.users',
		route: '/users',
		requiresUsers: true,
		group: 'ops',
	},
	{
		key: '/sessions',
		icon: PlugZap,
		label: 'sidebar.sessions',
		route: '/sessions',
		requiresUsers: true,
		group: 'ops',
	},
];
