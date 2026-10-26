<script lang="ts">
	import Skeleton from './Skeleton.svelte';
</script>

<div class="ui-page animate-fade-in">
	<!-- Header skeleton -->
	<div class="ui-card shadow-sm px-6 py-4">
		<Skeleton width="120px" height="1.25rem" rounded="md" />
	</div>

	<!-- Content skeleton -->
	<div class="flex-1 ui-card shadow-sm p-6">
		<div class="space-y-4">
			<div class="flex gap-4">
				<Skeleton width="80px" height="2rem" rounded="md" />
				<Skeleton width="80px" height="2rem" rounded="md" />
				<Skeleton width="80px" height="2rem" rounded="md" />
			</div>
			<div class="space-y-3 mt-6">
				{#each { length: 5 }, i (i)}
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

