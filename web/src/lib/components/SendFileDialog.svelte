<script lang="ts">
	import { appState } from '$lib/state.svelte';
	import { sendFiles, isTauri, isAndroid } from '$lib/api';
	import type { DiscoveredPeer } from '$lib/types';
	import { formatBytes, extractPasswordFromUrl } from '$lib/format';
	import { open } from '@tauri-apps/plugin-dialog';
	import {
		Send,
		FileText,
		Folder,
		FolderPlus,
		FilePlus,
		MessageSquare,
		ClipboardPaste,
		X,
		KeyRound,
		AlertCircle
	} from 'lucide-svelte';

	let { peer, onClose } = $props<{
		peer: DiscoveredPeer;
		onClose: () => void;
	}>();

	let pinInput = $state('');
	let isSending = $state(false);
	let errorMsg = $state<string | null>(null);
	let isComposingText = $state(false);
	let inlineTextMessage = $state('');

	async function pickFiles() {
		isComposingText = false;
		if (isTauri()) {
			try {
				const res = await open({
					multiple: true,
					directory: false
				});
				if (res) {
					const paths = Array.isArray(res) ? res : [res];
					appState.addFiles(paths);
				}
			} catch (e) {
				console.error('File pick dialog error:', e);
			}
		} else {
			appState.addFiles([`/home/user/Documents/test-document-${appState.preparedItems.length + 1}.pdf`]);
		}
	}

	async function pickFolder() {
		isComposingText = false;
		if (isAndroid()) {
			errorMsg = 'Folder selection is not supported on Android due to Scoped Storage. Please use Choose Files.';
			setTimeout(() => {
				errorMsg = null;
			}, 4000);
			return;
		}
		if (isTauri()) {
			try {
				const res = await open({
					directory: true,
					multiple: false
				});
				if (res && typeof res === 'string') {
					appState.addFolder(res);
				}
			} catch (e) {
				console.error('Folder pick dialog error:', e);
			}
		} else {
			appState.addFolder('/home/user/Projects/LanFlareDemo');
		}
	}

	async function pasteClipboard() {
		isComposingText = false;
		const res = await appState.pasteFromClipboard();
		if (!res.success) {
			errorMsg = res.message || 'Clipboard is empty or contains unsupported content';
			setTimeout(() => {
				if (errorMsg?.includes('Clipboard')) errorMsg = null;
			}, 3000);
		} else {
			errorMsg = null;
		}
	}

	async function handleSend() {
		if (isComposingText && inlineTextMessage.trim()) {
			await appState.addTextMessage(inlineTextMessage.trim());
			inlineTextMessage = '';
			isComposingText = false;
		}

		if (appState.preparedItems.length === 0) {
			errorMsg = 'Please add at least one file, folder, or text message to transfer';
			return;
		}

		isSending = true;
		errorMsg = null;

		try {
			const filePaths = appState.preparedItems.map((i) => i.path);
			const sessionId = await sendFiles({
				targetFingerprint: peer.fingerprint,
				targetEndpoint: peer.endpoints[0],
				files: filePaths,
				pin: pinInput.trim() ? extractPasswordFromUrl(pinInput.trim()) : undefined
			});

			appState.selectedPeer = peer;
			if (!isTauri()) {
				appState.transfers.unshift({
					id: sessionId,
					direction: 'outgoing',
					peerAlias: peer.alias,
					peerEndpoint: peer.endpoints[0] || '192.168.1.100:53317',
					files: appState.preparedItems.map((item) => ({
						id: item.path,
						name: item.name,
						size: item.size || 1048576,
						bytesSent: 0,
						status: 'pending',
						path: item.path
					})),
					totalBytes: appState.totalPreparedSize || 1048576,
					bytesTransferred: 0,
					speedBytesPerSec: 0,
					etaSeconds: null,
					status: 'waiting',
					startTime: Date.now()
				});
			}

			// Single recipient: Sends files to one recipient. Selection will be cleared after finished files transfer.
			appState.clearPreparedItems();
			appState.activeTab = 'transfers';
			onClose();
		} catch (e) {
			errorMsg = String(e);
		} finally {
			isSending = false;
		}
	}
</script>

<div
	class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 dark:bg-black/75 backdrop-blur-sm p-4 animate-in fade-in duration-150"
	style="padding-top: calc(1rem + var(--safe-top)); padding-bottom: calc(1rem + var(--safe-bottom)); padding-left: calc(1rem + var(--safe-left)); padding-right: calc(1rem + var(--safe-right));"
>
	<div
		class="w-full max-w-lg bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-800 rounded-2xl shadow-2xl p-6 space-y-5 animate-in zoom-in-95 duration-150 max-h-[90vh] flex flex-col"
	>
		<!-- Header -->
		<div class="flex items-center justify-between border-b border-slate-200 dark:border-slate-800 pb-3 flex-none">
			<div class="flex items-center gap-3">
				<div class="p-2.5 rounded-xl bg-indigo-500/10 border border-indigo-500/20 text-indigo-600 dark:text-indigo-400">
					<Send class="w-5 h-5" />
				</div>
				<div>
					<h3 class="text-base font-semibold text-slate-900 dark:text-slate-100">Send to Peer</h3>
					<p class="text-xs text-slate-500 dark:text-slate-400">
						Target: <strong class="text-slate-800 dark:text-slate-200">{peer.alias}</strong> ({peer.endpoints[0] || 'Peer'})
					</p>
				</div>
			</div>
			<button
				onclick={onClose}
				class="p-1 rounded-lg text-slate-400 hover:text-slate-700 dark:hover:text-slate-200 hover:bg-slate-100 dark:hover:bg-slate-800 transition-colors"
			>
				<X class="w-5 h-5" />
			</button>
		</div>

		<!-- Action Buttons: Files, Folder, Text, Clipboard -->
		<div class="space-y-3 flex-1 overflow-y-auto pr-1">
			<div class="grid grid-cols-2 {isAndroid() ? 'sm:grid-cols-3' : 'sm:grid-cols-4'} gap-2">
				<button
					type="button"
					onclick={pickFiles}
					class="flex flex-col items-center justify-center p-3 rounded-xl border border-slate-200 dark:border-slate-800 bg-slate-50 dark:bg-slate-950/50 hover:bg-indigo-50 dark:hover:bg-indigo-950/40 hover:border-indigo-300 dark:hover:border-indigo-600 text-slate-700 dark:text-slate-300 hover:text-indigo-600 dark:hover:text-indigo-400 transition-all cursor-pointer group"
				>
					<FilePlus class="w-5 h-5 mb-1 text-slate-500 group-hover:text-indigo-600 dark:group-hover:text-indigo-400" />
					<span class="text-xs font-semibold">Choose Files</span>
				</button>

				{#if !isAndroid()}
					<button
						type="button"
						onclick={pickFolder}
						class="flex flex-col items-center justify-center p-3 rounded-xl border border-slate-200 dark:border-slate-800 bg-slate-50 dark:bg-slate-950/50 hover:bg-indigo-50 dark:hover:bg-indigo-950/40 hover:border-indigo-300 dark:hover:border-indigo-600 text-slate-700 dark:text-slate-300 hover:text-indigo-600 dark:hover:text-indigo-400 transition-all cursor-pointer group"
					>
						<FolderPlus class="w-5 h-5 mb-1 text-slate-500 group-hover:text-indigo-600 dark:group-hover:text-indigo-400" />
						<span class="text-xs font-semibold">Choose Folder</span>
					</button>
				{/if}

				<button
					type="button"
					onclick={() => (isComposingText = !isComposingText)}
					class="flex flex-col items-center justify-center p-3 rounded-xl border transition-all cursor-pointer group {isComposingText
						? 'bg-indigo-50 dark:bg-indigo-950/60 border-indigo-400 dark:border-indigo-500 text-indigo-600 dark:text-indigo-400'
						: 'border-slate-200 dark:border-slate-800 bg-slate-50 dark:bg-slate-950/50 hover:bg-indigo-50 dark:hover:bg-indigo-950/40 hover:border-indigo-300 dark:hover:border-indigo-600 text-slate-700 dark:text-slate-300'}"
				>
					<MessageSquare class="w-5 h-5 mb-1 {isComposingText ? 'text-indigo-600 dark:text-indigo-400' : 'text-slate-500 group-hover:text-indigo-600 dark:group-hover:text-indigo-400'}" />
					<span class="text-xs font-semibold">Text Message</span>
				</button>

				<button
					type="button"
					onclick={pasteClipboard}
					class="flex flex-col items-center justify-center p-3 rounded-xl border border-slate-200 dark:border-slate-800 bg-slate-50 dark:bg-slate-950/50 hover:bg-indigo-50 dark:hover:bg-indigo-950/40 hover:border-indigo-300 dark:hover:border-indigo-600 text-slate-700 dark:text-slate-300 hover:text-indigo-600 dark:hover:text-indigo-400 transition-all cursor-pointer group"
				>
					<ClipboardPaste class="w-5 h-5 mb-1 text-slate-500 group-hover:text-indigo-600 dark:group-hover:text-indigo-400" />
					<span class="text-xs font-semibold">From Clipboard</span>
				</button>
			</div>

			<!-- Inline Text Input when Text Message is selected -->
			{#if isComposingText}
				<div class="space-y-1.5 p-3 rounded-xl bg-slate-50 dark:bg-slate-950/70 border border-indigo-200 dark:border-indigo-900/60 animate-in fade-in duration-150">
					<div class="flex items-center justify-between text-xs text-slate-500">
						<span class="font-medium text-slate-700 dark:text-slate-300">Message to {peer.alias}</span>
						<span class="text-[11px] text-slate-400">{new TextEncoder().encode(inlineTextMessage).length} bytes</span>
					</div>
					<textarea
						bind:value={inlineTextMessage}
						rows="4"
						placeholder="Type a message to {peer.alias}..."
						class="w-full bg-white dark:bg-slate-900 border border-slate-300 dark:border-slate-800 rounded-lg p-2.5 text-xs text-slate-900 dark:text-slate-100 placeholder-slate-400 focus:outline-none focus:border-indigo-500 font-sans resize-none"
					></textarea>
				</div>
			{/if}

			<!-- Selected Items List -->
			{#if appState.preparedItems.length > 0}
				<div class="space-y-1.5">
					<div class="flex items-center justify-between text-xs text-slate-500 px-0.5">
						<span>Selected items ({appState.preparedItems.length})</span>
						{#if appState.totalPreparedSize > 0}
							<span class="font-mono">{formatBytes(appState.totalPreparedSize)}</span>
						{/if}
					</div>
					<div class="bg-slate-50 dark:bg-slate-950/60 border border-slate-200 dark:border-slate-800 rounded-xl p-3 max-h-48 overflow-y-auto space-y-1.5 divide-y divide-slate-200 dark:divide-slate-800/60">
						{#each appState.preparedItems as item, idx}
							<div class="pt-1.5 first:pt-0 flex items-center justify-between text-xs">
								<div class="flex items-center gap-2 truncate max-w-[85%]">
									{#if item.type === 'folder'}
										<Folder class="w-3.5 h-3.5 text-amber-500 flex-shrink-0" />
									{:else if item.type === 'text'}
										<MessageSquare class="w-3.5 h-3.5 text-violet-500 flex-shrink-0" />
									{:else}
										<FileText class="w-3.5 h-3.5 text-indigo-600 dark:text-indigo-400 flex-shrink-0" />
									{/if}
									<span class="truncate text-slate-800 dark:text-slate-300" title={item.path}>{item.name}</span>
									{#if item.size > 0}
										<span class="text-[10px] text-slate-400 font-mono">({formatBytes(item.size)})</span>
									{/if}
								</div>
								<button
									onclick={() => appState.removePreparedItem(idx)}
									class="text-slate-400 hover:text-rose-600 dark:hover:text-rose-400 p-0.5 rounded transition-colors"
								>
									<X class="w-3.5 h-3.5" />
								</button>
							</div>
						{/each}
					</div>
				</div>
			{/if}

			<!-- Optional PIN Input -->
			<div class="space-y-1.5">
				<label for="target-pin-input" class="text-xs font-medium text-slate-700 dark:text-slate-300 flex items-center gap-1.5">
					<KeyRound class="w-3.5 h-3.5 text-slate-400" />
					<span>Receiver PIN (if required by receiver)</span>
				</label>
				<input
					id="target-pin-input"
					type="text"
					placeholder="Optional PIN (or paste receiver URL)"
					bind:value={pinInput}
					oninput={() => {
						pinInput = extractPasswordFromUrl(pinInput);
					}}
					class="w-full bg-slate-50 dark:bg-slate-950 border border-slate-300 dark:border-slate-800 rounded-lg px-3 py-2 text-xs text-slate-900 dark:text-slate-200 focus:outline-none focus:border-indigo-500 font-mono"
				/>
			</div>

			<!-- Error Banner -->
			{#if errorMsg}
				<div
					class="flex items-start gap-2 p-3 rounded-lg bg-rose-500/10 border border-rose-500/20 text-rose-600 dark:text-rose-400 text-xs"
				>
					<AlertCircle class="w-4 h-4 flex-shrink-0 mt-0.5" />
					<span>{errorMsg}</span>
				</div>
			{/if}
		</div>

		<!-- Footer -->
		<div class="flex items-center justify-end gap-3 pt-2 border-t border-slate-200 dark:border-slate-800 flex-none">
			<button
				onclick={onClose}
				class="py-2 px-4 rounded-xl text-slate-600 dark:text-slate-400 hover:text-slate-900 dark:hover:text-slate-200 text-xs font-medium transition-colors"
			>
				Cancel
			</button>
			<button
				onclick={handleSend}
				disabled={isSending || (appState.preparedItems.length === 0 && (!isComposingText || !inlineTextMessage.trim()))}
				class="flex items-center gap-2 py-2 px-5 rounded-xl bg-indigo-600 hover:bg-indigo-500 text-white font-medium text-xs shadow-md shadow-indigo-600/20 transition-all cursor-pointer disabled:opacity-50"
			>
				<Send class="w-4 h-4" />
				<span>{isSending ? 'Initiating...' : `Send to ${peer.alias}`}</span>
			</button>
		</div>
	</div>
</div>
