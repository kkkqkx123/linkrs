<script lang="ts">
  let { icon, label, onclick, disabled = false, variant = 'default' }: {
    icon: string;
    label?: string;
    onclick?: () => void;
    disabled?: boolean;
    variant?: 'default' | 'primary' | 'text';
  } = $props();

  const baseClass = 'inline-flex items-center justify-center gap-1 rounded transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed text-sm';
  const variants: Record<string, string> = {
    default: 'px-3 py-1.5 border border-gray-300 bg-white hover:bg-gray-50 text-gray-700',
    primary: 'px-3 py-1.5 bg-blue-500 hover:bg-blue-600 text-white',
    text: 'px-2 py-1 hover:bg-gray-100 text-gray-600',
  };
</script>

<button class="{baseClass} {variants[variant]}" {onclick} {disabled}>
  <span>{icon}</span>
  {#if label}<span>{label}</span>{/if}
</button>