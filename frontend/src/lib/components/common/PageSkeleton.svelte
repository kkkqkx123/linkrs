<script lang="ts">
  import Skeleton from './Skeleton.svelte';
</script>

<div class="flex flex-col h-full gap-4 animate-fade-in">
  <!-- Header skeleton -->
  <div class="bg-white dark:bg-[#1C2333] rounded-lg shadow-sm px-5 py-3">
    <Skeleton width="120px" height="1.25rem" rounded="md" />
  </div>

  <!-- Content skeleton -->
  <div class="flex-1 bg-white dark:bg-[#1C2333] rounded-lg shadow-sm p-6">
    <div class="space-y-4">
      <div class="flex gap-4">
        <Skeleton width="80px" height="2rem" rounded="md" />
        <Skeleton width="80px" height="2rem" rounded="md" />
        <Skeleton width="80px" height="2rem" rounded="md" />
      </div>
      <div class="space-y-3 mt-6">
        {#each [1, 2, 3, 4, 5] as _}
          <div class="flex gap-4">
            <Skeleton class="flex-1" height="0.75rem" rounded="sm" />
            <Skeleton width="60%" height="0.75rem" rounded="sm" />
            <Skeleton width="30%" height="0.75rem" rounded="sm" />
          </div>
        {/each}
      </div>
    </div>
  </div>
</div>

<style>
  @keyframes fade-in {
    from { opacity: 0; }
    to { opacity: 1; }
  }
  .animate-fade-in {
    animation: fade-in 0.3s ease-out;
  }
</style>