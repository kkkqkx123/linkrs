<script lang="ts">
  let { class: className = '', width = '100%', height = '1rem', rounded = 'md' }: {
    class?: string;
    width?: string;
    height?: string;
    rounded?: 'sm' | 'md' | 'lg' | 'xl' | 'full';
  } = $props();

  const roundMap: Record<string, string> = {
    sm: 'rounded-sm', md: 'rounded-md', lg: 'rounded-lg', xl: 'rounded-xl', full: 'rounded-full',
  };
</script>

<div
  class="animate-pulse bg-gray-200 dark:bg-gray-700 {roundMap[rounded]} {className}"
  style="width: {width}; height: {height}"
></div>