<script lang="ts">
  import { copyToClipboard } from '$utils/function';

  let { text, maxLength = 0 }: { text: string; maxLength?: number } = $props();
  let copied = $state(false);

  const display = $derived(maxLength > 0 && text.length > maxLength ? text.slice(0, maxLength) + '...' : text);

  async function handleCopy() {
    const success = await copyToClipboard(text);
    if (success) {
      copied = true;
      setTimeout(() => copied = false, 2000);
    }
  }
</script>

<span class="inline-flex items-center gap-1 group">
  <span class="font-mono text-xs">{display}</span>
  <button
    class="opacity-0 group-hover:opacity-100 cursor-pointer text-xs text-blue-500 hover:text-blue-700 transition-opacity"
    onclick={handleCopy}
  >
    {copied ? '✓' : '📋'}
  </button>
</span>