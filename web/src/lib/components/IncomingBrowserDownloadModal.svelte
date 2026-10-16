<script lang="ts">
	import { appState } from '$lib/state.svelte';
	import { formatBytes } from '$lib/format';
	import { Globe, Check, X, FileDown } from 'lucide-svelte';

	// Prompt when user is outside of Share via link view or wants global notification
	let currentRequest = $derived(
		appState.sendMode !== 'link' && appState.pendingBrowserDownloads.length > 0
			? appState.pendingBrowserDownloads[0]
			: null
	);
</script>

{#if currentRequest}
	<!-- Modal Backdrop -->
	<div
		class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 dark:bg-black/75 backdrop-blur-sm p-4 animate-in fade-in duration-150"
	>
		<!-- Dialog Container -->
		<div
			class="w-full max-w-md bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-800 rounded-2xl shadow-2xl p-6 space-y-5 animate-in zoom-in-95 duration-150"
		>
			<!-- Header -->
			<div class="flex items-center gap-3">
				<div class="p-3 rounded-xl bg-indigo-500/10 border border-indigo-500/20 text-indigo-600 dark:text-indigo-400">
					<Globe class="w-6 h-6" />
				</div>
				<div>
					<h3 class="text-base font-semibold text-slate-900 dark:text-slate-100">Browser Download Request</h3>
					<p class="text-xs text-slate-500 dark:text-slate-400">
						From IP <strong class="text-slate-800 dark:text-slate-200">{currentRequest.remoteIp}</strong>
					</p>
				</div>
			</div>

			<!-- Requested File Box -->
			<div class="bg-slate-50 dark:bg-slate-950/60 border border-slate-200 dark:border-slate-800 rounded-xl p-3.5 space-y-2">
				<div class="flex items-center justify-between text-xs text-slate-500 dark:text-slate-400 font-medium">
					<span>Requested File</span>
					{#if currentRequest.fileSize > 0}
						<span class="font-mono text-slate-800 dark:text-slate-300">{formatBytes(currentRequest.fileSize)}</span>
					{/if}
				</div>

				<div class="flex items-center gap-2.5 text-xs text-slate-800 dark:text-slate-200">
					<FileDown class="w-4 h-4 text-indigo-500 flex-shrink-0" />
					<span class="truncate font-medium" title={currentRequest.fileName}>{currentRequest.fileName}</span>
				</div>
			</div>

			<p class="text-xs text-slate-600 dark:text-slate-400 leading-relaxed">
				A web browser has requested to download this file. Do you want to accept and stream the file?
			</p>

			<!-- Action Buttons -->
			<div class="flex items-center gap-3 pt-2">
				<button
					onclick={() => appState.respondBrowserDownload(currentRequest.downloadId, false)}
					class="flex-1 flex items-center justify-center gap-2 py-2.5 px-4 rounded-xl bg-slate-150 hover:bg-slate-200 dark:bg-slate-800 dark:hover:bg-slate-750 text-slate-700 dark:text-slate-300 hover:text-slate-900 dark:hover:text-white font-medium text-xs transition-colors cursor-pointer border border-slate-250 dark:border-slate-700/60"
				>
					<X class="w-4 h-4" />
					<span>Decline</span>
				</button>
				<button
					onclick={() => appState.respondBrowserDownload(currentRequest.downloadId, true)}
					class="flex-1 flex items-center justify-center gap-2 py-2.5 px-4 rounded-xl bg-indigo-600 hover:bg-indigo-500 text-white font-medium text-xs shadow-lg shadow-indigo-600/20 transition-all cursor-pointer"
				>
					<Check class="w-4 h-4" />
					<span>Accept Download</span>
				</button>
			</div>
		</div>
	</div>
{/if}
