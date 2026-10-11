<script lang="ts">
	import { appState } from '$lib/state.svelte';
	import { sendFiles, isTauri } from '$lib/api';
	import type { DiscoveredPeer } from '$lib/types';
	import { open } from '@tauri-apps/plugin-dialog';
	import { Send, UploadCloud, File, Plus, X, KeyRound, AlertCircle } from 'lucide-svelte';

	let { peer, onClose } = $props<{
		peer: DiscoveredPeer;
		onClose: () => void;
	}>();

	let selectedFilePaths = $state<string[]>([]);
	let pinInput = $state('');
	let isSending = $state(false);
	let errorMsg = $state<string | null>(null);

	async function pickFiles() {
		if (isTauri()) {
			try {
				const res = await open({
					multiple: true,
					directory: false
				});
				if (res) {
					if (Array.isArray(res)) {
						selectedFilePaths = [...new Set([...selectedFilePaths, ...res])];
					} else {
						selectedFilePaths = [...new Set([...selectedFilePaths, res])];
					}
				}
			} catch (e) {
				console.error('File pick dialog error:', e);
			}
		} else {
			// Mock in browser
			selectedFilePaths = [
				...selectedFilePaths,
				`/home/user/Documents/test-document-${selectedFilePaths.length + 1}.pdf`
			];
		}
	}

	function removeFile(index: number) {
		selectedFilePaths = selectedFilePaths.filter((_, i) => i !== index);
	}

	async function handleSend() {
		if (selectedFilePaths.length === 0) {
			errorMsg = 'Please add at least one file to transfer';
			return;
		}

		isSending = true;
		errorMsg = null;

		try {
			const sessionId = await sendFiles({
				targetFingerprint: peer.fingerprint,
				files: selectedFilePaths,
				pin: pinInput.trim() ? pinInput.trim() : undefined
			});

			appState.selectedPeer = peer;
			onClose();
		} catch (e) {
			errorMsg = String(e);
		} finally {
			isSending = false;
		}
	}
</script>

<div
	class="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-sm p-4 animate-in fade-in duration-150"
>
	<div
		class="w-full max-w-lg bg-slate-900 border border-slate-800 rounded-2xl shadow-2xl p-6 space-y-6 animate-in zoom-in-95 duration-150"
	>
		<!-- Header -->
		<div class="flex items-center justify-between border-b border-slate-800 pb-4">
			<div class="flex items-center gap-3">
				<div class="p-2.5 rounded-xl bg-indigo-500/10 border border-indigo-500/20 text-indigo-400">
					<Send class="w-5 h-5" />
				</div>
				<div>
					<h3 class="text-base font-semibold text-slate-100">Send Files</h3>
					<p class="text-xs text-slate-400">
						Target: <strong class="text-slate-200">{peer.alias}</strong> ({peer.endpoints[0] || 'Peer'})
					</p>
				</div>
			</div>
			<button
				onclick={onClose}
				class="p-1 rounded-lg text-slate-400 hover:text-slate-200 hover:bg-slate-800 transition-colors"
			>
				<X class="w-5 h-5" />
			</button>
		</div>

		<!-- File Drop / Selection Zone -->
		<div class="space-y-3">
			<button
				onclick={pickFiles}
				type="button"
				class="w-full border-2 border-dashed border-slate-800 hover:border-indigo-500/60 rounded-xl p-6 flex flex-col items-center justify-center gap-2 bg-slate-950/40 hover:bg-slate-950/80 transition-all cursor-pointer group"
			>
				<UploadCloud class="w-8 h-8 text-slate-500 group-hover:text-indigo-400 transition-colors" />
				<div class="text-center">
					<p class="text-xs font-semibold text-slate-300 group-hover:text-white">
						Click to choose files
					</p>
					<p class="text-[11px] text-slate-500">Pick any local documents, photos, or archives</p>
				</div>
			</button>

			<!-- Selected Files List -->
			{#if selectedFilePaths.length > 0}
				<div class="bg-slate-950/60 border border-slate-800 rounded-xl p-3 max-h-44 overflow-y-auto space-y-1.5 divide-y divide-slate-800/60">
					{#each selectedFilePaths as path, idx}
						{@const fileName = path.split('/').pop() || path}
						<div class="pt-1.5 first:pt-0 flex items-center justify-between text-xs">
							<div class="flex items-center gap-2 truncate max-w-[85%]">
								<File class="w-3.5 h-3.5 text-indigo-400 flex-shrink-0" />
								<span class="truncate text-slate-300" title={path}>{fileName}</span>
							</div>
							<button
								onclick={() => removeFile(idx)}
								class="text-slate-500 hover:text-rose-400 p-0.5 rounded transition-colors"
							>
								<X class="w-3.5 h-3.5" />
							</button>
						</div>
					{/each}
				</div>
			{/if}
		</div>

		<!-- Optional PIN Input -->
		<div class="space-y-1.5">
			<label for="target-pin-input" class="text-xs font-medium text-slate-300 flex items-center gap-1.5">
				<KeyRound class="w-3.5 h-3.5 text-slate-400" />
				<span>Receiver PIN (if required by receiver)</span>
			</label>
			<input
				id="target-pin-input"
				type="text"
				placeholder="Optional PIN"
				bind:value={pinInput}
				class="w-full bg-slate-950 border border-slate-800 rounded-lg px-3 py-2 text-xs text-slate-200 focus:outline-none focus:border-indigo-500 font-mono"
			/>
		</div>

		<!-- Error Banner -->
		{#if errorMsg}
			<div
				class="flex items-start gap-2 p-3 rounded-lg bg-rose-500/10 border border-rose-500/20 text-rose-400 text-xs"
			>
				<AlertCircle class="w-4 h-4 flex-shrink-0 mt-0.5" />
				<span>{errorMsg}</span>
			</div>
		{/if}

		<!-- Footer -->
		<div class="flex items-center justify-end gap-3 pt-2 border-t border-slate-800">
			<button
				onclick={onClose}
				class="py-2 px-4 rounded-xl text-slate-400 hover:text-slate-200 text-xs font-medium transition-colors"
			>
				Cancel
			</button>
			<button
				onclick={handleSend}
				disabled={isSending || selectedFilePaths.length === 0}
				class="flex items-center gap-2 py-2 px-5 rounded-xl bg-indigo-600 hover:bg-indigo-500 text-white font-medium text-xs shadow-md shadow-indigo-600/20 transition-all cursor-pointer disabled:opacity-50"
			>
				<Send class="w-4 h-4" />
				<span>{isSending ? 'Initiating...' : `Send ${selectedFilePaths.length} File${selectedFilePaths.length === 1 ? '' : 's'}`}</span>
			</button>
		</div>
	</div>
</div>
