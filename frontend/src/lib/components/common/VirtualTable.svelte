<script lang="ts">
  import { onDestroy } from 'svelte';
  import { computeWindow, estimateColumnWidths } from '$utils/virtualWindow';
  import { formatCellValue } from '$utils/parseData';

  export interface IndexedRow {
    index: number;
    row: Record<string, unknown>;
  }

  let {
    columns = [],
    rows = [],
    rowHeight = 32,
    height = 400,
  }: {
    columns: string[];
    rows: IndexedRow[];
    rowHeight?: number;
    height?: number;
  } = $props();

  let containerEl = $state<HTMLDivElement | undefined>(undefined);
  let scrollTop = $state(0);
  let viewportHeight = $state(0);
  let raf = 0;

  $effect(() => {
    viewportHeight = containerEl?.clientHeight ?? height;
  });

  let frozenKey = $state('');
  let widths = $state<number[]>([]);

  // Freeze column widths once the schema arrives so appended rows never
  // shift the layout while the stream is still flowing.
  $effect(() => {
    const cols = columns;
    if (cols.length === 0) return;
    const key = cols.join('\u0000');
    if (key === frozenKey) return;
    frozenKey = key;
    widths = estimateColumnWidths(
      cols,
      rows.slice(0, 50).map((entry) => entry.row),
      formatCellValue,
    );
  });

  let viewport = $derived(computeWindow(rows.length, scrollTop, viewportHeight, rowHeight, 1));
  let visible = $derived(rows.slice(viewport.start, viewport.end));

  function onScroll() {
    const el = containerEl;
    if (!el || raf) return;
    const top = el.scrollTop;
    const client = el.clientHeight;
    raf = requestAnimationFrame(() => {
      raf = 0;
      scrollTop = top;
      viewportHeight = client;
    });
  }

  onDestroy(() => {
    if (raf) cancelAnimationFrame(raf);
  });
</script>

<div
  class="overflow-auto border border-gray-200 dark:border-gray-700 rounded"
  style="height: {height}px;"
  bind:this={containerEl}
  onscroll={onScroll}
>
  <table class="w-full text-sm border-collapse" style="table-layout: fixed;">
    <colgroup>
      <col style="width: 64px;" />
      {#each widths as width}
        <col style="width: {width}px;" />
      {/each}
    </colgroup>
    <thead>
      <tr class="bg-gray-50 dark:bg-gray-800/50">
        <th
          class="px-3 py-2 text-left font-medium text-gray-400 border-b border-gray-200 dark:border-gray-700 whitespace-nowrap sticky top-0 bg-gray-50 dark:bg-gray-800 z-10"
          >#</th
        >
        {#each columns as column}
          <th
            class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700 whitespace-nowrap overflow-hidden text-ellipsis sticky top-0 bg-gray-50 dark:bg-gray-800 z-10"
            title={column}>{column}</th
          >
        {/each}
      </tr>
    </thead>
    <tbody>
      {#if viewport.topSpacer > 0}
        <tr><td colspan={columns.length + 1} style="height: {viewport.topSpacer}px; padding: 0; border: 0;"></td></tr>
      {/if}
      {#each visible as entry (entry.index)}
        <tr
          class="hover:bg-gray-50 dark:hover:bg-gray-800/30 even:bg-gray-50/50 dark:even:bg-gray-800/20"
          style="height: {rowHeight}px;"
        >
          <td class="px-3 py-1 border-b border-gray-100 dark:border-gray-700/50 text-gray-400 text-xs whitespace-nowrap overflow-hidden">{entry.index + 1}</td>
          {#each columns as column}
            <td
              class="px-3 py-1 border-b border-gray-100 dark:border-gray-700/50 text-gray-700 dark:text-gray-300 whitespace-nowrap overflow-hidden text-ellipsis"
              title={formatCellValue(entry.row[column])}>{formatCellValue(entry.row[column])}</td
            >
          {/each}
        </tr>
      {/each}
      {#if viewport.bottomSpacer > 0}
        <tr><td colspan={columns.length + 1} style="height: {viewport.bottomSpacer}px; padding: 0; border: 0;"></td></tr>
      {/if}
    </tbody>
  </table>
</div>
