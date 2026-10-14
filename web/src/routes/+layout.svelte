<script lang="ts">
	import '../app.css';
	import { onMount } from 'svelte';
	import { themeManager } from '$lib/theme.svelte';

	let { children } = $props();

	onMount(() => {
		themeManager.init();
		return () => {
			themeManager.destroy();
		};
	});
</script>

<div class="flex flex-col h-screen w-screen overflow-hidden bg-slate-50 dark:bg-slate-950 text-slate-800 dark:text-slate-100 select-none transition-colors duration-150">
	{@render children()}
</div>
