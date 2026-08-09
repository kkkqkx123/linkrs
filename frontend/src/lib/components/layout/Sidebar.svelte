<script lang="ts">
  import { t } from 'svelte-i18n';
  import { useLocation, navigate } from 'svelte-routing';

  const location = useLocation();
  const path = $derived($location.pathname);

  const menuItems = [
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
        { key: '/schema/spaces', icon: '◈', label: 'sidebar.spaces', route: '/schema/spaces' },
        { key: '/schema/tags', icon: '🏷', label: 'sidebar.tags', route: '/schema/tags' },
        { key: '/schema/edges', icon: '↔', label: 'sidebar.edges', route: '/schema/edges' },
        { key: '/schema/indexes', icon: '🔍', label: 'sidebar.indexes', route: '/schema/indexes' },
        { key: '/schema/visualization', icon: '👁', label: 'sidebar.visualization', route: '/schema/visualization' },
        { key: '/schema/stats', icon: '📊', label: 'sidebar.stats', route: '/schema/stats' },
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
  ];

  function isActive(item: { key: string; route?: string; children?: Array<{ key: string }> }): boolean {
    if (item.route && path.startsWith(item.route)) return true;
    if (item.children) return item.children.some(c => path.startsWith(c.key));
    return false;
  }

  function handleNav(route?: string) {
    if (route) navigate(route);
  }
</script>

<aside class="w-60 bg-white dark:bg-[#1C2333] border-r border-gray-200 dark:border-gray-700/50 flex flex-col flex-shrink-0 overflow-y-auto transition-colors duration-300">
  <div class="px-5 py-4 border-b border-gray-200 dark:border-gray-700/50">
    <h1 class="text-lg font-bold text-[var(--color-primary)]">GraphDB</h1>
  </div>
  <nav class="flex-1 p-3">
    <ul class="space-y-1">
      {#each menuItems as item}
        {#if item.children}
          <li>
            <details open={item.children.some(c => path.startsWith(c.key))}>
              <summary class="flex items-center gap-2 px-3 py-2 rounded-md cursor-pointer text-sm font-medium text-gray-600 dark:text-gray-400 hover:bg-gray-100 dark:hover:bg-gray-700/50">
                <span>{item.icon}</span>
                <span>{$t(item.label)}</span>
              </summary>
              <ul class="ml-4 mt-1 space-y-1">
                {#each item.children as child}
                  <li>
                    <button
                      class="w-full flex items-center gap-2 px-3 py-1.5 rounded-md text-sm cursor-pointer transition-colors {isActive(child) ? 'bg-blue-50 dark:bg-blue-900/30 text-[var(--color-primary)] font-medium' : 'text-gray-600 dark:text-gray-400 hover:bg-gray-100 dark:hover:bg-gray-700/50'}"
                      onclick={() => handleNav(child.route)}
                    >
                      <span>{child.icon}</span>
                      <span>{$t(child.label)}</span>
                    </button>
                  </li>
                {/each}
              </ul>
            </details>
          </li>
        {:else}
          <li>
            <button
              class="w-full flex items-center gap-2 px-3 py-2 rounded-md text-sm cursor-pointer transition-colors {isActive(item) ? 'bg-blue-50 dark:bg-blue-900/30 text-[var(--color-primary)] font-medium' : 'text-gray-600 dark:text-gray-400 hover:bg-gray-100 dark:hover:bg-gray-700/50'}"
              onclick={() => handleNav(item.route)}
            >
              <span>{item.icon}</span>
              <span>{$t(item.label)}</span>
            </button>
          </li>
        {/if}
      {/each}
    </ul>
  </nav>
</aside>