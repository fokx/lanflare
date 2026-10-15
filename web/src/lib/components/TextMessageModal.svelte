<script lang="ts">
	import { appState } from '$lib/state.svelte';
	import { MessageSquare, X, Plus } from 'lucide-svelte';

	let textContent = $state('');
	let isSubmitting = $state(false);

	async function handleAdd() {
		if (!textContent.trim()) return;
		isSubmitting = true;
		try {
			await appState.addTextMessage(textContent.trim());
			textContent = '';
			appState.showTextModal = false;
		} catch (e) {
			console.error('Failed to add text message:', e);
		} finally {
			isSubmitting = false;
		}
	}
</script>

{#if appState.showTextModal}
	<div
		class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 dark:bg-black/75 backdrop-blur-sm p-4 animate-in fade-in duration-150"
	>
		<div
			class="w-full max-w-lg bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-800 rounded-2xl shadow-2xl p-6 space-y-4 animate-in zoom-in-95 duration-150"
		>
			<!-- Header -->
			<div class="flex items-center justify-between border-b border-slate-200 dark:border-slate-800 pb-3">
				<div class="flex items-center gap-3">
					<div class="p-2 rounded-xl bg-indigo-500/10 text-indigo-600 dark:text-indigo-400">
						<MessageSquare class="w-5 h-5" />
					</div>
					<div>
						<h3 class="text-base font-semibold text-slate-900 dark:text-slate-100">Send Text</h3>
						<p class="text-xs text-slate-500 dark:text-slate-400">Type or paste text to share</p>
					</div>
				</div>
				<button
					onclick={() => (appState.showTextModal = false)}
					class="p-1 rounded-lg text-slate-400 hover:text-slate-700 dark:hover:text-slate-200 hover:bg-slate-100 dark:hover:bg-slate-800 transition-colors cursor-pointer"
				>
					<X class="w-5 h-5" />
				</button>
			</div>

			<!-- Single Simple Message Box -->
			<div class="space-y-1">
				<textarea
					bind:value={textContent}
					rows="6"
					placeholder="Type a message..."
					class="w-full bg-slate-50 dark:bg-slate-950 border border-slate-300 dark:border-slate-800 rounded-xl p-3 text-sm text-slate-900 dark:text-slate-100 placeholder-slate-400 focus:outline-none focus:border-indigo-500 focus:ring-1 focus:ring-indigo-500 font-sans resize-none"
					onkeydown={(e) => {
						if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
							e.preventDefault();
							handleAdd();
						}
					}}
				></textarea>
				<div class="flex justify-between text-[11px] text-slate-400 dark:text-slate-500 px-1">
					<span>Press Ctrl+Enter to add</span>
					<span>{new TextEncoder().encode(textContent).length} bytes</span>
				</div>
			</div>

			<!-- Footer -->
			<div class="flex items-center justify-end gap-3 pt-2 border-t border-slate-200 dark:border-slate-800">
				<button
					onclick={() => (appState.showTextModal = false)}
					class="py-2 px-4 rounded-xl text-slate-600 dark:text-slate-400 hover:text-slate-900 dark:hover:text-slate-200 text-xs font-medium transition-colors cursor-pointer"
				>
					Cancel
				</button>
				<button
					onclick={handleAdd}
					disabled={isSubmitting || !textContent.trim()}
					class="flex items-center gap-2 py-2 px-5 rounded-xl bg-indigo-600 hover:bg-indigo-500 text-white font-medium text-xs shadow-md shadow-indigo-600/20 transition-all cursor-pointer disabled:opacity-50"
				>
					<Plus class="w-4 h-4" />
					<span>Add</span>
				</button>
			</div>
		</div>
	</div>
{/if}
