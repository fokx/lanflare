<script lang="ts">
	import { appState } from '$lib/state.svelte';
	import { formatBytes } from '$lib/format';
	import { ShieldAlert, Check, X, FileText, ArrowDownLeft } from 'lucide-svelte';
</script>

{#if appState.pendingIncoming}
	{@const payload = appState.pendingIncoming}
	<!-- Modal Backdrop -->
	<div
		class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 dark:bg-black/75 backdrop-blur-sm p-4 animate-in fade-in duration-150"
		style="padding-top: calc(1rem + var(--safe-top)); padding-bottom: calc(1rem + var(--safe-bottom)); padding-left: calc(1rem + var(--safe-left)); padding-right: calc(1rem + var(--safe-right));"
	>
		<!-- Dialog Container -->
		<div
			class="w-full max-w-md bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-800 rounded-2xl shadow-2xl p-6 space-y-5 animate-in zoom-in-95 duration-150"
		>
			<!-- Top Header -->
			<div class="flex items-center gap-3">
				<div class="p-3 rounded-xl bg-amber-500/10 border border-amber-500/20 text-amber-600 dark:text-amber-400">
					<ArrowDownLeft class="w-6 h-6" />
				</div>
				<div>
					<h3 class="text-base font-semibold text-slate-900 dark:text-slate-100">Incoming File Transfer</h3>
					<p class="text-xs text-slate-500 dark:text-slate-400">
						From <strong class="text-slate-800 dark:text-slate-200">{payload.senderAlias}</strong> ({payload.senderIp})
					</p>
				</div>
			</div>

			<!-- Files Summary -->
			<div class="bg-slate-50 dark:bg-slate-950/60 border border-slate-200 dark:border-slate-800 rounded-xl p-3.5 space-y-2">
				<div class="flex items-center justify-between text-xs text-slate-500 dark:text-slate-400 font-medium">
					<span>{payload.files.length} file{payload.files.length > 1 ? 's' : ''} offered</span>
					<span class="font-mono text-slate-800 dark:text-slate-300">{formatBytes(payload.totalBytes)}</span>
				</div>

				<div class="max-h-40 overflow-y-auto space-y-1.5 pr-1 divide-y divide-slate-200 dark:divide-slate-800/60">
					{#each payload.files as file (file.id)}
						<div class="pt-1.5 first:pt-0 flex items-center justify-between text-xs">
							<div class="flex items-center gap-2 truncate max-w-[70%]">
								<FileText class="w-3.5 h-3.5 text-slate-400 dark:text-slate-500 flex-shrink-0" />
								<span class="truncate text-slate-800 dark:text-slate-300" title={file.fileName}>{file.fileName}</span>
							</div>
							<span class="font-mono text-slate-500 dark:text-slate-400 text-[11px]">{formatBytes(file.size)}</span>
						</div>
					{/each}
				</div>
			</div>

			<p class="text-xs text-slate-600 dark:text-slate-400 leading-relaxed">
				Do you want to accept and download these files directly to your configured directory?
			</p>

			<!-- Action Buttons -->
			<div class="flex items-center gap-3 pt-2">
				<button
					onclick={() => appState.declineIncoming()}
					class="flex-1 flex items-center justify-center gap-2 py-2.5 px-4 rounded-xl bg-slate-150 hover:bg-slate-200 dark:bg-slate-800 dark:hover:bg-slate-750 text-slate-700 dark:text-slate-300 hover:text-slate-900 dark:hover:text-white font-medium text-xs transition-colors cursor-pointer border border-slate-250 dark:border-slate-700/60"
				>
					<X class="w-4 h-4" />
					<span>Decline</span>
				</button>
				<button
					onclick={() => appState.acceptIncoming()}
					class="flex-1 flex items-center justify-center gap-2 py-2.5 px-4 rounded-xl bg-indigo-600 hover:bg-indigo-500 text-white font-medium text-xs shadow-lg shadow-indigo-600/20 transition-all cursor-pointer"
				>
					<Check class="w-4 h-4" />
					<span>Accept Transfer</span>
				</button>
			</div>
		</div>
	</div>
{/if}
