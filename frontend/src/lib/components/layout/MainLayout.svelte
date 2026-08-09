<script lang="ts">
  import Header from './Header.svelte';
  import Sidebar from './Sidebar.svelte';

  let { children } = $props();
</script>

<div class="flex h-screen overflow-hidden bg-gray-50 dark:bg-[#0B0F17] transition-colors duration-300">
  <Sidebar />
  <div class="flex flex-col flex-1 min-w-0">
    <Header />
    <main class="flex-1 overflow-auto p-6 bg-gray-50 dark:bg-[#0B0F17] transition-colors duration-300">
      {@render children()}
    </main>
  </div>
</div>