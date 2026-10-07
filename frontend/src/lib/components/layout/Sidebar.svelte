<script lang="ts">
	import { page } from '$app/state';
	import { t, type MessageKey } from '$i18n';
	import { canManageUsers } from '$stores/connection';
	import { fromStore } from 'svelte/store';

	const path = $derived(page.url.pathname);
	const managesUsers = fromStore(canManageUsers);

	interface MenuItem {
		key: string;
		icon: string;
		label: MessageKey;
		route?: string;
		requiresUsers?: boolean;
		children?: MenuItem[];
	}

	const menuItems: MenuItem[] = [
		{
			key: '/console',
			icon: '⌨',
			label: 'sidebar.console',
			route: '/console',
		},
		{
			key: 'schema',
			icon: '🗄',
			label: 'sidebar.schema',
			children: [
				{
					key: '/schema/spaces',
					icon: '◈',
					label: 'sidebar.spaces',
					route: '/schema/spaces',
				},
				{
					key: '/schema/tags',
					icon: '🏷',
					label: 'sidebar.tags',
					route: '/schema/tags',
				},
				{
					key: '/schema/edges',
					icon: '↔',
					label: 'sidebar.edges',
					route: '/schema/edges',
				},
				{
					key: '/schema/indexes',
					icon: '🔍',
					label: 'sidebar.indexes',
					route: '/schema/indexes',
				},
				{
					key: '/schema/visualization',
					icon: '👁',
					label: 'sidebar.visualization',
					route: '/schema/visualization',
				},
				{
					key: '/schema/functions',
					icon: 'ƒ',
					label: 'sidebar.functions',
					route: '/schema/functions',
				},
				{
					key: '/schema/versions',
					icon: '🕘',
					label: 'sidebar.versions',
					route: '/schema/versions',
				},
			],
		},
		{
			key: '/graph',
			icon: '🔗',
			label: 'sidebar.graph',
			route: '/graph',
		},
		{
			key: '/data-browser',
			icon: '📋',
			label: 'sidebar.dataBrowser',
			route: '/data-browser',
		},
		{
			key: '/monitoring',
			icon: '📈',
			label: 'sidebar.monitoring',
			route: '/monitoring',
		},
		{
			key: '/vector',
			icon: '🧭',
			label: 'sidebar.vector',
			route: '/vector',
		},
		{
			key: '/fulltext',
			icon: '🔎',
			label: 'sidebar.fulltext',
			route: '/fulltext',
		},
		{
			key: '/migration',
			icon: '🚚',
			label: 'sidebar.migration',
			route: '/migration',
		},
		{
			key: '/batch',
			icon: '📦',
			label: 'sidebar.batch',
			route: '/batch',
		},
		{
			key: '/transfer',
			icon: '⇄',
			label: 'sidebar.transfer',
			route: '/transfer',
		},
		{
			key: '/users',
			icon: '👥',
			label: 'sidebar.users',
			route: '/users',
			requiresUsers: true,
		},
	];

	const visibleItems = $derived(
		menuItems.filter((item) => !item.requiresUsers || managesUsers.current),
	);

	function isActive(item: MenuItem): boolean {
		if (item.route && path.startsWith(item.route)) return true;
		if (item.children) return item.children.some((c) => path.startsWith(c.key));
		return false;
	}
</script>

<aside
	class="w-60 bg-white dark:bg-[#1C2333] border-r border-gray-200 dark:border-gray-700/50 flex flex-col flex-shrink-0 overflow-y-auto transition-colors duration-300"
>
	<div class="px-5 py-4 border-b border-gray-200 dark:border-gray-700/50">
		<h1 class="text-lg font-bold text-[var(--color-primary)]">GraphDB</h1>
	</div>
	<nav class="flex-1 p-3">
		<ul class="space-y-1">
			{#each visibleItems as item (item.key)}
				{#if item.children}
					<li>
						<details open={item.children.some((c) => path.startsWith(c.key))}>
							<summary
								class="flex items-center gap-2 px-3 py-2 rounded-md cursor-pointer text-sm font-medium text-gray-600 dark:text-gray-400 hover:bg-gray-100 dark:hover:bg-gray-700/50"
							>
								<span>{item.icon}</span>
								<span>{t(item.label)}</span>
							</summary>
							<ul class="ml-4 mt-1 space-y-1">
								{#each item.children as child (child.key)}
									<li>
										<a
											href={child.route}
											class="w-full flex items-center gap-2 px-3 py-1.5 rounded-md text-sm transition-colors {isActive(
												child,
											)
												? 'bg-blue-50 dark:bg-blue-900/30 text-[var(--color-primary)] font-medium'
												: 'text-gray-600 dark:text-gray-400 hover:bg-gray-100 dark:hover:bg-gray-700/50'}"
										>
											<span>{child.icon}</span>
											<span>{t(child.label)}</span>
										</a>
									</li>
								{/each}
							</ul>
						</details>
					</li>
				{:else}
					<li>
						<a
							href={item.route}
							class="w-full flex items-center gap-2 px-3 py-2 rounded-md text-sm transition-colors {isActive(
								item,
							)
								? 'bg-blue-50 dark:bg-blue-900/30 text-[var(--color-primary)] font-medium'
								: 'text-gray-600 dark:text-gray-400 hover:bg-gray-100 dark:hover:bg-gray-700/50'}"
						>
							<span>{item.icon}</span>
							<span>{t(item.label)}</span>
						</a>
					</li>
				{/if}
			{/each}
		</ul>
	</nav>
</aside>
