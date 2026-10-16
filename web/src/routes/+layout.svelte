<script lang="ts">
	import '../app.css';
	import { onMount } from 'svelte';
	import { themeManager } from '$lib/theme.svelte';

	let { children } = $props();

	onMount(() => {
		themeManager.init();

		// Synchronously read Android safe insets if injected by native webview bridge
		const syncAndroidInsets = () => {
			if (typeof window !== 'undefined' && 'AndroidSafeInsets' in window) {
				try {
					const bridge = (window as any).AndroidSafeInsets;
					if (bridge) {
						const top = bridge.getTop?.();
						const bottom = bridge.getBottom?.();
						const left = bridge.getLeft?.();
						const right = bridge.getRight?.();
						if (typeof top === 'number' && top > 0) document.documentElement.style.setProperty('--safe-area-inset-top', `${top}px`);
						if (typeof bottom === 'number' && bottom > 0) document.documentElement.style.setProperty('--safe-area-inset-bottom', `${bottom}px`);
						if (typeof left === 'number' && left > 0) document.documentElement.style.setProperty('--safe-area-inset-left', `${left}px`);
						if (typeof right === 'number' && right > 0) document.documentElement.style.setProperty('--safe-area-inset-right', `${right}px`);
					}

				} catch (_) {}
			}
		};

		syncAndroidInsets();
		const timer = setTimeout(syncAndroidInsets, 100);

		return () => {
			clearTimeout(timer);
			themeManager.destroy();
		};
	});
</script>

<div class="flex flex-col h-screen h-[100dvh] w-screen overflow-hidden bg-slate-50 dark:bg-slate-950 text-slate-800 dark:text-slate-100 select-none transition-colors duration-150">
	{@render children()}
</div>
