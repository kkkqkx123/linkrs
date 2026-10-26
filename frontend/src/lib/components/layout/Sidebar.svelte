<script lang="ts">
	import { page } from '$app/state';
	import { t, type MessageKey } from '$i18n';
	import { canManageUsers } from '$stores/connection';
	import { fromStore } from 'svelte/store';

	const path = $derived(page.url.pathname);
	const managesUsers = fromStore(canManageUsers);

	// ---- 侧栏宽度:可拖拽调节,持久化到 localStorage ----
	// Menu content (icon + label + padding) needs ~150px; the default leaves
	// even breathing room while staying well under the old oversized width.
	const SIDEBAR_DEFAULT = 192;
	const SIDEBAR_MIN = 168;
	const SIDEBAR_MAX = 288;
	const WIDTH_KEY = 'linkrs.sidebar-width';

	function loadWidth(): number {
		const saved = Number(localStorage.getItem(WIDTH_KEY));
		if (Number.isFinite(saved) && saved >= SIDEBAR_MIN && saved <= SIDEBAR_MAX) {
			return saved;
		}
		return SIDEBAR_DEFAULT;
	}

	let width = $state(loadWidth());
	let resizing = $state(false);

	function startResize(e: PointerEvent) {
		e.preventDefault();
		resizing = true;
		document.body.style.cursor = 'col-resize';
		document.body.style.userSelect = 'none';
		const startX = e.clientX;
		const startWidth = width;

		function onMove(ev: PointerEvent) {
			width = Math.min(
				SIDEBAR_MAX,
				Math.max(SIDEBAR_MIN, startWidth + (ev.clientX - startX)),
			);
		}
		function onUp() {
			resizing = false;
			document.body.style.cursor = '';
			document.body.style.userSelect = '';
			localStorage.setItem(WIDTH_KEY, String(width));
			window.removeEventListener('pointermove', onMove);
			window.removeEventListener('pointerup', onUp);
		}
		window.addEventListener('pointermove', onMove);
		window.addEventListener('pointerup', onUp);
	}

	function resetWidth() {
		width = SIDEBAR_DEFAULT;
		localStorage.removeItem(WIDTH_KEY);
	}

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
		{
			key: '/sessions',
			icon: '🔌',
			label: 'sidebar.sessions',
			route: '/sessions',
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

	// The graph canvas (cytoscape) is lazily imported on first use; hovering a
	// graph-using entry prefetches that chunk so the first render does not wait
	// on an 800 KiB download.
	const GRAPH_ROUTES = ['/graph', '/schema', '/console'];
	let prefetchedGraph = false;
	function prefetchGraphChunk(item: MenuItem): void {
		if (prefetchedGraph) return;
		const route = item.route ?? item.key;
		if (!GRAPH_ROUTES.some((prefix) => route.startsWith(prefix))) return;
		prefetchedGraph = true;
		void import('cytoscape');
	}
</script>

<div class="flex flex-shrink-0 relative" style="width: {width}px">
	<aside
		class="w-full bg-white dark:bg-[#1C2333] border-r border-gray-200 dark:border-gray-700/50 flex flex-col overflow-y-auto transition-colors duration-300 {resizing
			? 'pointer-events-none select-none'
			: ''}"
	>
		<div class="px-5 py-4 border-b border-gray-200 dark:border-gray-700/50">
			<h1 class="text-lg font-bold text-[var(--color-primary)]">Linkrs</h1>
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
											onpointerenter={() => prefetchGraphChunk(child)}
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
							onpointerenter={() => prefetchGraphChunk(item)}
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
	<div
		role="separator"
		aria-orientation="vertical"
		title="{t('sidebar.resizeHint')} ({t('common.doubleClickReset')})"
		class="absolute top-0 right-[-2px] w-1 h-full cursor-col-resize touch-none z-10 {resizing
			? 'bg-[var(--color-primary)]/40'
			: 'bg-transparent hover:bg-[var(--color-primary)]/25'} transition-colors"
		onpointerdown={startResize}
		ondblclick={resetWidth}
	></div>
</div>
