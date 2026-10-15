<script lang="ts">
	import { appState } from '$lib/state.svelte';
	import { formatBytes, shortenFingerprint } from '$lib/format';
	import type { DiscoveredPeer } from '$lib/types';
	import { open } from '@tauri-apps/plugin-dialog';
	import { isTauri } from '$lib/api';
	import ShareViaLinkView from '$lib/components/ShareViaLinkView.svelte';
	import {
		Laptop,
		Smartphone,
		Server,
		Globe,
		Radio,
		ShieldCheck,
		Search,
		Send,
		FileText,
		Folder,
		FolderPlus,
		FilePlus,
		MessageSquare,
		ClipboardPaste,
		Link,
		Users,
		User,
		Check,
		X,
		KeyRound
	} from 'lucide-svelte';

	let { onSelectPeer } = $props<{
		onSelectPeer: (peer: DiscoveredPeer) => void;
	}>();

	let pinInput = $state('');
	let isMultipleSending = $state(false);

	function getDeviceIcon(deviceType?: string) {
		switch (deviceType?.toLowerCase()) {
			case 'mobile':
				return Smartphone;
			case 'server':
				return Server;
			case 'web':
				return Globe;
			case 'desktop':
			default:
				return Laptop;
		}
	}

	async function pickFiles() {
		if (isTauri()) {
			try {
				const res = await open({ multiple: true, directory: false });
				if (res) {
					const paths = Array.isArray(res) ? res : [res];
					appState.addFiles(paths);
				}
			} catch (e) {
				console.error('File dialog error:', e);
			}
		} else {
			appState.addFiles([`/home/user/Documents/test-document-${appState.preparedItems.length + 1}.pdf`]);
		}
	}

	async function pickFolder() {
		if (isTauri()) {
			try {
				const res = await open({ directory: true, multiple: false });
				if (res && typeof res === 'string') {
					appState.addFolder(res);
				}
			} catch (e) {
				console.error('Folder dialog error:', e);
			}
		} else {
			appState.addFolder('/home/user/Projects/LanFlareDemo');
		}
	}

	async function pasteClipboard() {
		await appState.pasteFromClipboard();
	}

	async function handleSendPreparedToPeer(peer: DiscoveredPeer) {
		if (appState.preparedItems.length === 0) {
			onSelectPeer(peer);
		} else {
			try {
				await appState.sendPreparedToPeer(peer);
			} catch (e) {
				console.error('Send error:', e);
			}
		}
	}

	async function handleSendMultiple() {
		if (appState.preparedItems.length === 0 || appState.selectedRecipients.length === 0) return;
		isMultipleSending = true;
		try {
			await appState.sendPreparedToMultiple(pinInput.trim() || undefined);
		} finally {
			isMultipleSending = false;
		}
	}
</script>

{#if appState.sendMode === 'link'}
	<ShareViaLinkView />
{:else}
	<div class="flex flex-col h-full overflow-hidden p-4 sm:p-6 gap-4 sm:gap-5">
		<!-- Top Bar: Title & Search -->
		<div class="flex flex-col sm:flex-row sm:items-center justify-between gap-3 flex-none">
			<div class="flex items-center gap-3">
				<div class="p-2 sm:p-2.5 rounded-xl bg-indigo-500/10 border border-indigo-500/20 text-indigo-600 dark:text-indigo-400">
					<Radio class="w-5 h-5 sm:w-6 sm:h-6 animate-pulse" />
				</div>
				<div>
					<h2 class="text-lg sm:text-xl font-bold text-slate-900 dark:text-slate-100 flex items-center gap-2">
						Discovered Devices
						<span
							class="text-xs px-2 py-0.5 rounded-full bg-slate-200 dark:bg-slate-800 text-slate-700 dark:text-slate-400 border border-slate-300 dark:border-slate-700/60 font-mono"
						>
							{appState.peers.length}
						</span>
					</h2>
					<p class="text-[11px] sm:text-xs text-slate-500 dark:text-slate-400">
						Active local peers on Multicast UDP & mDNS DNS-SD
					</p>
				</div>
			</div>

			<!-- Search Input -->
			<div class="relative w-full sm:w-64">
				<Search class="absolute left-3 top-2.5 w-4 h-4 text-slate-400 pointer-events-none" />
				<input
					type="text"
					placeholder="Filter devices or IP..."
					bind:value={appState.searchQuery}
					class="w-full bg-white dark:bg-slate-900 border border-slate-300 dark:border-slate-800 rounded-lg pl-9 pr-3 py-1.5 text-sm text-slate-900 dark:text-slate-200 placeholder-slate-400 dark:placeholder-slate-500 focus:outline-none focus:border-indigo-500 transition-colors shadow-sm dark:shadow-none"
				/>
			</div>
		</div>

		<!-- Mode Switcher Tabs (Single, Multiple, Share via link) -->
		<div class="bg-white dark:bg-slate-900/90 border border-slate-200 dark:border-slate-800 rounded-2xl p-2.5 space-y-2.5 flex-none shadow-sm">
			<div class="flex flex-col sm:flex-row sm:items-center justify-between gap-2 border-b border-slate-100 dark:border-slate-800/80 pb-2">
				<div class="flex items-center gap-1.5 bg-slate-100 dark:bg-slate-950 p-1 rounded-xl">
					<button
						onclick={() => (appState.sendMode = 'single')}
						class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-semibold transition-all cursor-pointer {appState.sendMode === 'single'
							? 'bg-white dark:bg-slate-800 text-indigo-600 dark:text-indigo-400 shadow-sm'
							: 'text-slate-600 dark:text-slate-400 hover:text-slate-900 dark:hover:text-white'}"
					>
						<User class="w-3.5 h-3.5" />
						<span>Single recipient</span>
					</button>

					<button
						onclick={() => (appState.sendMode = 'multiple')}
						class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-semibold transition-all cursor-pointer {appState.sendMode === 'multiple'
							? 'bg-white dark:bg-slate-800 text-indigo-600 dark:text-indigo-400 shadow-sm'
							: 'text-slate-600 dark:text-slate-400 hover:text-slate-900 dark:hover:text-white'}"
					>
						<Users class="w-3.5 h-3.5" />
						<span>Multiple recipients</span>
						{#if appState.selectedRecipients.length > 0}
							<span class="px-1.5 py-0.2 rounded-full text-[10px] font-mono bg-indigo-100 dark:bg-indigo-900/60 text-indigo-700 dark:text-indigo-300">
								{appState.selectedRecipients.length}
							</span>
						{/if}
					</button>

					<button
						onclick={() => {
							appState.sendMode = 'link';
							if (appState.preparedItems.length > 0) {
								appState.startShareViaLink();
							}
						}}
						class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg text-xs font-semibold transition-all cursor-pointer {(appState.sendMode as string) === 'link'
							? 'bg-white dark:bg-slate-800 text-indigo-600 dark:text-indigo-400 shadow-sm'
							: 'text-slate-600 dark:text-slate-400 hover:text-slate-900 dark:hover:text-white'}"
					>
						<Link class="w-3.5 h-3.5" />
						<span>Share via link</span>
					</button>
				</div>

				<!-- Quick Add Content Buttons -->
				<div class="flex items-center gap-1.5 flex-wrap">
					<button
						onclick={pickFiles}
						class="flex items-center gap-1 py-1 px-2.5 rounded-lg bg-slate-50 dark:bg-slate-800 hover:bg-slate-100 dark:hover:bg-slate-700 text-slate-700 dark:text-slate-300 text-xs font-medium border border-slate-200 dark:border-slate-700/60 transition-colors cursor-pointer"
					>
						<FilePlus class="w-3.5 h-3.5 text-indigo-500" />
						<span>Add Files</span>
					</button>
					<button
						onclick={pickFolder}
						class="flex items-center gap-1 py-1 px-2.5 rounded-lg bg-slate-50 dark:bg-slate-800 hover:bg-slate-100 dark:hover:bg-slate-700 text-slate-700 dark:text-slate-300 text-xs font-medium border border-slate-200 dark:border-slate-700/60 transition-colors cursor-pointer"
					>
						<FolderPlus class="w-3.5 h-3.5 text-amber-500" />
						<span>Add Folder</span>
					</button>
					<button
						onclick={() => (appState.showTextModal = true)}
						class="flex items-center gap-1 py-1 px-2.5 rounded-lg bg-slate-50 dark:bg-slate-800 hover:bg-slate-100 dark:hover:bg-slate-700 text-slate-700 dark:text-slate-300 text-xs font-medium border border-slate-200 dark:border-slate-700/60 transition-colors cursor-pointer"
					>
						<MessageSquare class="w-3.5 h-3.5 text-violet-500" />
						<span>Text</span>
					</button>
					<button
						onclick={pasteClipboard}
						class="flex items-center gap-1 py-1 px-2.5 rounded-lg bg-slate-50 dark:bg-slate-800 hover:bg-slate-100 dark:hover:bg-slate-700 text-slate-700 dark:text-slate-300 text-xs font-medium border border-slate-200 dark:border-slate-700/60 transition-colors cursor-pointer"
					>
						<ClipboardPaste class="w-3.5 h-3.5 text-emerald-500" />
						<span>Paste</span>
					</button>
				</div>
			</div>

			<!-- Prepared Items Bar (if any) -->
			{#if appState.preparedItems.length > 0}
				<div class="flex items-center justify-between gap-2 text-xs">
					<div class="flex items-center gap-2 overflow-x-auto py-0.5 max-w-[85%] scrollbar-none">
						<span class="text-[11px] font-semibold text-slate-500 flex-shrink-0">
							Prepared ({appState.preparedItems.length}):
						</span>
						{#each appState.preparedItems as item, idx}
							<div class="flex items-center gap-1.5 px-2 py-0.8 rounded-lg bg-indigo-50 dark:bg-indigo-950/40 border border-indigo-200 dark:border-indigo-800/60 text-slate-800 dark:text-slate-200 flex-shrink-0">
								{#if item.type === 'folder'}
									<Folder class="w-3 h-3 text-amber-500" />
								{:else if item.type === 'text'}
									<MessageSquare class="w-3 h-3 text-violet-500" />
								{:else}
									<FileText class="w-3 h-3 text-indigo-500" />
								{/if}
								<span class="truncate max-w-[120px]">{item.name}</span>
								<button
									onclick={() => appState.removePreparedItem(idx)}
									class="p-0.5 text-slate-400 hover:text-rose-500 cursor-pointer"
								>
									<X class="w-3 h-3" />
								</button>
							</div>
						{/each}
					</div>

					<button
						onclick={() => appState.clearPreparedItems()}
						class="text-[11px] text-slate-400 hover:text-rose-500 transition-colors cursor-pointer flex-shrink-0 font-medium"
					>
						Clear All
					</button>
				</div>
			{:else}
				<p class="text-[11px] text-slate-400 dark:text-slate-500">
					{#if appState.sendMode === 'single'}
						Single recipient: Sends files to one recipient. Selection will be cleared after finished files transfer.
					{:else}
						Multiple recipients: Sends files to multiple recipients. Selection will not be cleared after finished files transfer.
					{/if}
				</p>
			{/if}
		</div>

		<!-- Peers Grid -->
		<div class="flex-1 overflow-y-auto pr-1">
			{#if appState.filteredPeers.length === 0}
				<div
					class="h-full flex flex-col items-center justify-center border border-dashed border-slate-300 dark:border-slate-800 rounded-2xl p-6 sm:p-8 text-center text-slate-400 dark:text-slate-500 gap-3"
				>
					<Radio class="w-10 h-10 sm:w-12 sm:h-12 text-slate-400 dark:text-slate-600 stroke-[1.5]" />
					<div class="space-y-1">
						<p class="text-sm font-medium text-slate-600 dark:text-slate-400">Scanning local network...</p>
						<p class="text-xs text-slate-400 dark:text-slate-500 max-w-sm">
							Make sure other LocalSend or LanFlare devices are connected to the same Wi-Fi or subnet.
						</p>
					</div>
				</div>
			{:else}
				<div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-3 sm:gap-4 pb-2">
					{#each appState.filteredPeers as peer (peer.fingerprint)}
						{@const Icon = getDeviceIcon(peer.deviceType)}
						{@const isSelected = appState.selectedRecipients.includes(peer.fingerprint)}

						<div
							class="group relative bg-white dark:bg-slate-900/90 border rounded-xl p-4 transition-all duration-200 flex flex-col justify-between shadow-sm hover:shadow-md dark:shadow-none {appState.sendMode === 'multiple' && isSelected
								? 'border-indigo-500 ring-2 ring-indigo-500/30 bg-indigo-50/20 dark:bg-indigo-950/20'
								: 'border-slate-200 dark:border-slate-800 hover:border-indigo-500/40'}"
						>
							<!-- Card Top -->
							<div class="flex items-start justify-between gap-2.5">
								<div class="flex items-center gap-3">
									{#if appState.sendMode === 'multiple'}
										<input
											type="checkbox"
											checked={isSelected}
											onchange={() => appState.toggleRecipient(peer.fingerprint)}
											class="w-4 h-4 accent-indigo-600 rounded cursor-pointer"
										/>
									{/if}
									<div
										class="p-2 sm:p-2.5 rounded-lg bg-slate-100 dark:bg-slate-800 border border-slate-200 dark:border-slate-700/60 text-slate-700 dark:text-slate-300 group-hover:text-indigo-600 dark:group-hover:text-indigo-400 group-hover:bg-indigo-50 dark:group-hover:bg-indigo-500/10 group-hover:border-indigo-200 dark:group-hover:border-indigo-500/20 transition-colors"
									>
										<Icon class="w-5 h-5" />
									</div>
									<div>
										<h3 class="font-semibold text-slate-900 dark:text-slate-200 text-sm leading-snug group-hover:text-indigo-600 dark:group-hover:text-white transition-colors truncate max-w-[140px] sm:max-w-none">
											{peer.alias}
										</h3>
										<p class="text-xs text-slate-500 dark:text-slate-400 truncate max-w-[140px] sm:max-w-none">
											{peer.deviceModel || peer.deviceType || 'Device'}
										</p>
									</div>
								</div>

								<!-- Security Badge -->
								<div
									class="flex items-center gap-1 text-[10px] sm:text-[11px] px-1.5 sm:px-2 py-0.5 rounded-md bg-emerald-500/10 border border-emerald-500/20 text-emerald-600 dark:text-emerald-400 font-medium flex-shrink-0"
									title="Mutual TLS verified with pinned SHA-256 fingerprint"
								>
									<ShieldCheck class="w-3.5 h-3.5" />
									<span>mTLS</span>
								</div>
							</div>

							<!-- Card Endpoints & Info -->
							<div class="my-3 pt-2.5 border-t border-slate-100 dark:border-slate-800/80 space-y-1 text-xs">
								<div class="flex items-center justify-between text-slate-500 dark:text-slate-400">
									<span>Endpoint:</span>
									<span class="font-mono text-slate-800 dark:text-slate-300 text-[11px]">
										{peer.endpoints[0] || 'Unknown IP'}
									</span>
								</div>
								<div class="flex items-center justify-between text-slate-500 dark:text-slate-400">
									<span>Fingerprint:</span>
									<span
										class="font-mono text-slate-500 dark:text-slate-400 hover:text-slate-800 dark:hover:text-slate-200 transition-colors cursor-help text-[11px]"
										title={peer.fingerprint}
									>
										{shortenFingerprint(peer.fingerprint)}
									</span>
								</div>
							</div>

							<!-- Card Bottom Action -->
							{#if appState.sendMode === 'single'}
								<button
									onclick={() => handleSendPreparedToPeer(peer)}
									class="w-full mt-1 flex items-center justify-center gap-2 py-2 px-3 rounded-lg bg-indigo-600 hover:bg-indigo-500 active:bg-indigo-700 text-white font-medium text-xs shadow-sm hover:shadow-indigo-500/20 transition-all cursor-pointer"
								>
									<Send class="w-3.5 h-3.5" />
									<span>{appState.preparedItems.length > 0 ? `Send ${appState.preparedItems.length} Item${appState.preparedItems.length === 1 ? '' : 's'}` : 'Send Files'}</span>
								</button>
							{:else}
								<button
									onclick={() => appState.toggleRecipient(peer.fingerprint)}
									class="w-full mt-1 flex items-center justify-center gap-1.5 py-2 px-3 rounded-lg border font-medium text-xs transition-all cursor-pointer {isSelected
										? 'bg-indigo-600 text-white border-indigo-600 shadow-sm'
										: 'bg-slate-50 dark:bg-slate-800 text-slate-700 dark:text-slate-300 border-slate-200 dark:border-slate-700 hover:bg-slate-100 dark:hover:bg-slate-700/80'}"
								>
									{#if isSelected}
										<Check class="w-3.5 h-3.5" />
										<span>Selected</span>
									{:else}
										<span>Select Recipient</span>
									{/if}
								</button>
							{/if}
						</div>
					{/each}
				</div>
			{/if}
		</div>

		<!-- Multiple Recipients Floating Bottom Bar -->
		{#if appState.sendMode === 'multiple' && appState.selectedRecipients.length > 0}
			<div
				class="bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-800 rounded-2xl p-4 shadow-xl flex flex-col sm:flex-row sm:items-center justify-between gap-3 animate-in slide-in-from-bottom-2 duration-150 flex-none"
			>
				<div class="flex items-center gap-3">
					<div class="p-2 rounded-xl bg-indigo-500/10 text-indigo-600 dark:text-indigo-400">
						<Users class="w-5 h-5" />
					</div>
					<div>
						<h4 class="font-semibold text-sm text-slate-900 dark:text-slate-100">
							{appState.selectedRecipients.length} Recipient{appState.selectedRecipients.length === 1 ? '' : 's'} Selected
						</h4>
						<p class="text-xs text-slate-500">
							{#if appState.preparedItems.length > 0}
								Ready to send {appState.preparedItems.length} item(s) &bull; Selection will be retained after transfer
							{:else}
								Choose files, folder, or text message above to send
							{/if}
						</p>
					</div>
				</div>

				<div class="flex items-center gap-2">
					<button
						onclick={() => appState.clearSelectedRecipients()}
						class="py-2 px-3 rounded-xl text-slate-600 dark:text-slate-400 hover:text-slate-900 dark:hover:text-slate-200 text-xs font-medium transition-colors cursor-pointer"
					>
						Deselect All
					</button>

					<button
						onclick={handleSendMultiple}
						disabled={isMultipleSending || appState.preparedItems.length === 0}
						class="flex items-center gap-2 py-2 px-5 rounded-xl bg-indigo-600 hover:bg-indigo-500 text-white font-medium text-xs shadow-md shadow-indigo-600/20 transition-all cursor-pointer disabled:opacity-50"
					>
						<Send class="w-4 h-4" />
						<span>{isMultipleSending ? 'Sending...' : `Send to ${appState.selectedRecipients.length} Recipient${appState.selectedRecipients.length === 1 ? '' : 's'}`}</span>
					</button>
				</div>
			</div>
		{/if}
	</div>
{/if}
