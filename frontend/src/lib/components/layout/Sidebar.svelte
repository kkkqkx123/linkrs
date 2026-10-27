<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { t, type MessageKey } from '$i18n';
	import { canManageUsers } from '$stores/connection';
	import { fromStore } from 'svelte/store';
	import type { Icon } from 'lucide-svelte';
	import {
		Keyboard,
		Database,
		Link2,
		Table,
		LineChart,
		Route,
		Package,
		Repeat,
		Users,
		PlugZap,
	} from 'lucide-svelte';

	const path = $derived(page.url.pathname);
	const managesUsers = fromStore(canManageUsers);

	// ---- 侧栏宽度:可拖拽调节,持久化到 localStorage ----
	// 下限是物理诚实值:图标约 20px + gap 8px + 链接 px-3 24px + 导航 p-3 24px.
	// 到达下限时图标完整显示、标签自然收为零,不留死区.
	// 上限 300 容纳长标签不换行;默认 176 只是回退,首次加载以实测贴合宽度为准.
	const SIDEBAR_DEFAULT = 176;
	const SIDEBAR_MIN = 76;
	const SIDEBAR_MAX = 300;
	const SNAP_PX = 6;
	const WIDTH_KEY = 'linkrs.sidebar-width.v2';

	function loadWidth(): number {
		const saved = Number(localStorage.getItem(WIDTH_KEY));
		if (Number.isFinite(saved)) {
			return Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, saved));
		}
		return SIDEBAR_DEFAULT;
	}

	let width = $state(loadWidth());
	let resizing = $state(false);
	let navEl: HTMLElement | null = null;

	function persist() {
		localStorage.setItem(WIDTH_KEY, String(Math.round(width)));
	}

	// 边缘贴合文字:最宽一行的完整内容宽度 + 导航内边距.
	// 标签始终保留在 DOM 中只做省略裁剪,因此 scrollWidth 就是文字全宽.
	function measureFitWidth(): number {
		if (!navEl) return SIDEBAR_DEFAULT;
		let maxRow = 0;
		navEl.querySelectorAll('a').forEach((row) => {
			maxRow = Math.max(maxRow, (row as HTMLElement).scrollWidth);
		});
		const cs = getComputedStyle(navEl);
		const pad = parseFloat(cs.paddingLeft) + parseFloat(cs.paddingRight);
		return Math.min(
			SIDEBAR_MAX,
			Math.max(SIDEBAR_MIN, Math.ceil(maxRow + pad) + 1),
		);
	}

	function fitWidth() {
		width = measureFitWidth();
		persist();
	}

	onMount(() => {
		// 无存档时不使用拍脑袋的默认值,直接贴合当前语言下的内容宽度.
		// 必须等字体就绪再测量,否则按回退字体量出的宽度偏小,标签会被截断.
		if (localStorage.getItem(WIDTH_KEY) === null) {
			document.fonts.ready.then(() => {
				width = measureFitWidth();
			});
		}
	});

	function startResize(e: PointerEvent) {
		e.preventDefault();
		resizing = true;
		document.body.style.cursor = 'col-resize';
		document.body.style.userSelect = 'none';
		const startX = e.clientX;
		const startWidth = width;
		const fit = measureFitWidth();

		function onMove(ev: PointerEvent) {
			let next = startWidth + (ev.clientX - startX);
			if (Math.abs(next - fit) <= SNAP_PX) next = fit;
			width = Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, next));
		}
		function onUp() {
			resizing = false;
			document.body.style.cursor = '';
			document.body.style.userSelect = '';
			persist();
			window.removeEventListener('pointermove', onMove);
			window.removeEventListener('pointerup', onUp);
		}
		window.addEventListener('pointermove', onMove);
		window.addEventListener('pointerup', onUp);
	}

	interface MenuItem {
		key: string;
		icon: typeof Icon;
		label: MessageKey;
		route?: string;
		requiresUsers?: boolean;
	}

	const menuItems: MenuItem[] = [
		{
			key: '/console',
			icon: Keyboard,
			label: 'sidebar.console',
			route: '/console',
		},
		{
			key: '/schema',
			icon: Database,
			label: 'sidebar.schema',
			route: '/schema',
		},
		{
			key: '/graph',
			icon: Link2,
			label: 'sidebar.graph',
			route: '/graph',
		},
		{
			key: '/data-browser',
			icon: Table,
			label: 'sidebar.dataBrowser',
			route: '/data-browser',
		},
		{
			key: '/monitoring',
			icon: LineChart,
			label: 'sidebar.monitoring',
			route: '/monitoring',
		},
		{
			key: '/migration',
			icon: Route,
			label: 'sidebar.migration',
			route: '/migration',
		},
		{
			key: '/batch',
			icon: Package,
			label: 'sidebar.batch',
			route: '/batch',
		},
		{
			key: '/transfer',
			icon: Repeat,
			label: 'sidebar.transfer',
			route: '/transfer',
		},
		{
			key: '/users',
			icon: Users,
			label: 'sidebar.users',
			route: '/users',
			requiresUsers: true,
		},
		{
			key: '/sessions',
			icon: PlugZap,
			label: 'sidebar.sessions',
			route: '/sessions',
			requiresUsers: true,
		},
	];

	const visibleItems = $derived(
		menuItems.filter((item) => !item.requiresUsers || managesUsers.current),
	);

	function isActive(item: MenuItem): boolean {
		return Boolean(item.route) && path.startsWith(item.route!);
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

<div
	class="flex flex-shrink-0 relative"
	style="width: {width}px; min-width: {SIDEBAR_MIN}px"
>
	<aside
		class="w-full bg-nav text-nav-text flex flex-col overflow-y-auto overflow-x-hidden transition-colors duration-300 {resizing
			? 'pointer-events-none select-none'
			: ''}"
	>
		<div class="px-5 py-5 border-b border-nav-border">
			<p
				class="text-base font-bold text-nav-brand truncate whitespace-nowrap overflow-hidden"
				title="Linkrs"
			>
				Linkrs
			</p>
			<p class="text-xs mt-1.5 text-nav-muted">
				{t('app.title')}
			</p>
		</div>
		<nav bind:this={navEl} class="flex-1 py-2">
			<ul class="ui-nav">
				{#each visibleItems as item (item.key)}
					<li>
						<a
							href={item.route}
							title={t(item.label)}
							onpointerenter={() => prefetchGraphChunk(item)}
							class="flex items-center gap-2 overflow-hidden whitespace-nowrap {isActive(
								item,
							)
								? 'active'
								: ''}"
						>
							<!-- 固定宽度图标列,保证各行文字起始位置对齐 -->
							<span class="flex-shrink-0 w-5 text-center flex items-center justify-center">
								<item.icon size={18} />
							</span>
							<span class="min-w-0 flex-1 truncate">{t(item.label)}</span>
						</a>
					</li>
				{/each}
			</ul>
		</nav>
	</aside>
	<div
		role="separator"
		aria-orientation="vertical"
		aria-valuenow={Math.round(width)}
		aria-valuemin={SIDEBAR_MIN}
		aria-valuemax={SIDEBAR_MAX}
		title="{t('sidebar.resizeHint')} ({t('sidebar.fitHint')})"
		class="absolute top-0 right-[-6px] w-3 h-full cursor-col-resize touch-none z-10 flex justify-center {resizing
			? 'bg-primary/40'
			: 'bg-transparent hover:bg-primary/25'} transition-colors"
		onpointerdown={startResize}
		ondblclick={fitWidth}
	>
		<div class="w-1 h-full bg-current opacity-0"></div>
	</div>
</div>
