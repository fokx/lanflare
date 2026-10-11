<script lang="ts">
	import '../app.css';
	let { children } = $props();
</script>

<div class="flex flex-col h-screen w-screen overflow-hidden bg-slate-950 text-slate-100 select-none">
	{@render children()}
</div>
