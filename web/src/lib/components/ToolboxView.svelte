<script lang="ts">
	import { onMount } from 'svelte';
	import { appState } from '$lib/state.svelte';
	import { shortenFingerprint } from '$lib/format';
	import type {
		SearchHitDto,
		UniversalClipboardPayloadDto,
		ScratchpadEditDto,
		AtomIdDto
	} from '$lib/types';
	import {
		Wrench,
		Clipboard,
		Search,
		FileCode,
		Share2,
		Copy,
		Check,
		ExternalLink,
		Download,
		Lock,
		Unlock,
		Laptop,
		Smartphone,
		Server,
		Radio,
		RefreshCw,
		Sparkles,
		ShieldCheck,
		Layers,
		CheckCircle2
	} from 'lucide-svelte';

	let activeSubTab = $state<'clipboard' | 'search' | 'scratchpad'>('clipboard');

	// Clipboard Ring State
	let manualClipboardText = $state('');
	let copiedIndex = $state<number | null>(null);
	let isBroadcastingClipboard = $state(false);

	// Federated Search State
	let searchQuery = $state('');
	let searchPin = $state('');
	let hasSearched = $state(false);

	// Collaborative Scratchpad State
	let scratchpadDocId = $state('scratchpad-main');
	let scratchpadTitle = $state('Shared Project Notes.md');
	let localScratchpadText = $state('');
	let lamportClock = $state(1);
	let editCounter = $state(0);
	let isSyncingDoc = $state(false);
	let copiedDocSuccess = $state(false);

	onMount(async () => {
		await appState.loadClipboardState();
		await loadScratchpad();
	});

	// --- Clipboard Ring Methods ---
	async function handleBroadcastClipboard() {
		if (!manualClipboardText.trim()) return;
		isBroadcastingClipboard = true;
		try {
			await appState.broadcastClipboardText(manualClipboardText.trim());
			manualClipboardText = '';
		} catch (e) {
			console.error('Broadcast failed:', e);
		} finally {
			isBroadcastingClipboard = false;
		}
	}

	async function copyToClipboard(text: string, index: number) {
		try {
			await navigator.clipboard.writeText(text);
			copiedIndex = index;
			setTimeout(() => {
				copiedIndex = null;
			}, 2000);
		} catch (e) {
			console.error('Failed to copy to local clipboard:', e);
		}
	}

	// --- Federated Search Methods ---
	async function handleSearch() {
		if (!searchQuery.trim()) return;
		hasSearched = true;
		await appState.searchFederatedFiles(searchQuery, searchPin);
	}

	function handleSearchKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') {
			handleSearch();
		}
	}

	function formatBytes(bytes: number): string {
		if (!bytes || bytes === 0) return '0 B';
		const k = 1024;
		const sizes = ['B', 'KB', 'MB', 'GB'];
		const i = Math.floor(Math.log(bytes) / Math.log(k));
		return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i];
	}

	// --- Collaborative Scratchpad Methods ---
	async function loadScratchpad() {
		isSyncingDoc = true;
		try {
			await appState.loadCollabDoc(scratchpadDocId, scratchpadTitle);
			localScratchpadText = appState.collabContent;
			lamportClock = appState.collabDoc?.lamport || 1;
		} finally {
			isSyncingDoc = false;
		}
	}

	async function handleScratchpadInput(e: Event) {
		const target = e.target as HTMLTextAreaElement;
		const newText = target.value;
		const oldText = localScratchpadText;
		localScratchpadText = newText;

		// Calculate incremental edit
		lamportClock += 1;
		editCounter += 1;
		const authorFp = appState.status?.fingerprint || 'LOCAL_AUTHOR';

		// Diff simplification: find point of insertion or deletion
		if (newText.length > oldText.length) {
			// Find inserted character
			let diffIdx = 0;
			while (diffIdx < oldText.length && oldText[diffIdx] === newText[diffIdx]) {
				diffIdx++;
			}
			const char = newText[diffIdx] || ' ';
			const atomId: AtomIdDto = {
				lamport: lamportClock,
				authorFp,
				counter: editCounter
			};
			const afterId: AtomIdDto | undefined =
				diffIdx > 0 && appState.collabDoc?.atoms[diffIdx - 1]
					? appState.collabDoc.atoms[diffIdx - 1].id
					: undefined;

			const editDto: ScratchpadEditDto = {
				docId: scratchpadDocId,
				editId: `edit-${Date.now()}-${editCounter}`,
				authorFp,
				lamport: lamportClock,
				op: {
					type: 'insert',
					atomId,
					afterId,
					value: char
				},
				timestampMs: Date.now()
			};
			appState.applyCollabEdit(editDto);
		} else if (newText.length < oldText.length) {
			// Deletion
			let diffIdx = 0;
			while (diffIdx < newText.length && oldText[diffIdx] === newText[diffIdx]) {
				diffIdx++;
			}
			if (appState.collabDoc && appState.collabDoc.atoms[diffIdx]) {
				const atomToDelete = appState.collabDoc.atoms[diffIdx];
				const editDto: ScratchpadEditDto = {
					docId: scratchpadDocId,
					editId: `edit-${Date.now()}-${editCounter}`,
					authorFp,
					lamport: lamportClock,
					op: {
						type: 'delete',
						atomId: atomToDelete.id
					},
					timestampMs: Date.now()
				};
				appState.applyCollabEdit(editDto);
			}
		}
	}

	async function handleCopyDocText() {
		try {
			await navigator.clipboard.writeText(localScratchpadText);
			copiedDocSuccess = true;
			setTimeout(() => {
				copiedDocSuccess = false;
			}, 2000);
		} catch (e) {
			console.error('Failed to copy document:', e);
		}
	}
</script>

<div class="h-full w-full flex flex-col overflow-hidden bg-slate-50 dark:bg-slate-950">
	<!-- Top Sub-Tab Navigation Bar -->
	<header
		aria-label="Toolbox navigation"
		class="px-4 py-3 bg-white dark:bg-slate-900 border-b border-slate-200 dark:border-slate-800/80 flex items-center justify-between flex-none"
	>
		<div class="flex items-center gap-2">
			<div class="p-1.5 rounded-lg bg-indigo-50 dark:bg-indigo-500/15 text-indigo-600 dark:text-indigo-400">
				<Wrench class="w-4 h-4" />
			</div>
			<div>
				<h2 class="font-bold text-sm text-slate-900 dark:text-white flex items-center gap-1.5">
					LAN Toolbox
					<span class="text-[9px] font-mono px-1.5 py-0.2 rounded bg-indigo-50 dark:bg-indigo-950/60 text-indigo-600 dark:text-indigo-400 border border-indigo-200 dark:border-indigo-800/60">
						Decentralized
					</span>
				</h2>
			</div>
		</div>

		<!-- Sub-tabs -->
		<div class="flex items-center gap-1 bg-slate-100 dark:bg-slate-800/80 p-1 rounded-xl border border-slate-200/80 dark:border-slate-700/60">
			<button
				onclick={() => (activeSubTab = 'clipboard')}
				class="px-3 py-1 text-xs font-semibold rounded-lg transition-all flex items-center gap-1.5 {activeSubTab === 'clipboard'
					? 'bg-white dark:bg-slate-900 text-indigo-600 dark:text-indigo-400 shadow-xs'
					: 'text-slate-600 dark:text-slate-400 hover:text-slate-900 dark:hover:text-white'}"
			>
				<Clipboard class="w-3.5 h-3.5" />
				<span>Clipboard Ring</span>
			</button>

			<button
				onclick={() => (activeSubTab = 'search')}
				class="px-3 py-1 text-xs font-semibold rounded-lg transition-all flex items-center gap-1.5 {activeSubTab === 'search'
					? 'bg-white dark:bg-slate-900 text-indigo-600 dark:text-indigo-400 shadow-xs'
					: 'text-slate-600 dark:text-slate-400 hover:text-slate-900 dark:hover:text-white'}"
			>
				<Search class="w-3.5 h-3.5" />
				<span>Federated Search</span>
			</button>

			<button
				onclick={() => (activeSubTab = 'scratchpad')}
				class="px-3 py-1 text-xs font-semibold rounded-lg transition-all flex items-center gap-1.5 {activeSubTab === 'scratchpad'
					? 'bg-white dark:bg-slate-900 text-indigo-600 dark:text-indigo-400 shadow-xs'
					: 'text-slate-600 dark:text-slate-400 hover:text-slate-900 dark:hover:text-white'}"
			>
				<FileCode class="w-3.5 h-3.5" />
				<span>CRDT Scratchpad</span>
			</button>
		</div>
	</header>

	<!-- Main Canvas Area -->
	<div class="flex-1 overflow-y-auto p-4 sm:p-6">
		{#if activeSubTab === 'clipboard'}
			<!-- ============================================================== -->
			<!-- TAB 1: Universal Clipboard Ring -->
			<!-- ============================================================== -->
			<div class="max-w-4xl mx-auto space-y-6">
				<!-- Ring Overview Card -->
				<div class="p-5 rounded-2xl bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-800 shadow-xs space-y-4">
					<div class="flex flex-col sm:flex-row sm:items-center justify-between gap-3">
						<div class="flex items-center gap-3">
							<div class="p-2.5 rounded-xl bg-gradient-to-br from-indigo-500 to-purple-600 text-white shadow-md">
								<Share2 class="w-5 h-5" />
							</div>
							<div>
								<h3 class="font-bold text-sm text-slate-900 dark:text-white">
									{appState.clipboardState?.ringName || "My Devices Self-Trust Ring"}
								</h3>
								<p class="text-xs text-slate-500 font-mono">
									Ring ID: {appState.clipboardState?.ringId || 'ring-personal-uuid'}
								</p>
							</div>
						</div>

						<div class="flex items-center gap-2">
							<span class="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-full text-[11px] font-semibold bg-emerald-50 dark:bg-emerald-950/60 text-emerald-600 dark:text-emerald-400 border border-emerald-200 dark:border-emerald-800/60">
								<ShieldCheck class="w-3.5 h-3.5" />
								<span>Guarded by ChaCha20-Poly1305</span>
							</span>
						</div>
					</div>

					<!-- Echo Loop Suppression Notice -->
					<div class="p-3 rounded-xl bg-indigo-50/60 dark:bg-indigo-950/30 border border-indigo-100 dark:border-indigo-900/40 text-[11px] text-indigo-900 dark:text-indigo-200 flex items-center gap-2">
						<Radio class="w-4 h-4 text-indigo-600 dark:text-indigo-400 flex-none" />
						<span>
							<strong>3-Layer Echo Filter:</strong> Loopback self-suppression, SHA-256 content deduplication, and monotonic sequence numbers protect against broadcast storms.
						</span>
					</div>

					<!-- Broadcast Manual Text/Snippet -->
					<div class="space-y-2 pt-2 border-t border-slate-100 dark:border-slate-800">
						<label for="broadcast-clipboard-input" class="block text-xs font-semibold text-slate-700 dark:text-slate-300">
							Broadcast Content to All Paired Devices
						</label>
						<div class="flex gap-2">
							<input
								id="broadcast-clipboard-input"
								type="text"
								bind:value={manualClipboardText}
								placeholder="Paste text or link to sync universally across devices..."
								class="flex-1 px-3 py-2 text-xs rounded-xl bg-slate-100 dark:bg-slate-800 border border-slate-200 dark:border-slate-700 text-slate-900 dark:text-white placeholder-slate-400 focus:outline-hidden focus:border-indigo-500"
							/>
							<button
								onclick={handleBroadcastClipboard}
								disabled={!manualClipboardText.trim() || isBroadcastingClipboard}
								class="px-4 py-2 text-xs font-bold rounded-xl bg-indigo-600 hover:bg-indigo-700 disabled:opacity-40 text-white transition-colors flex items-center gap-1.5"
							>
								<Share2 class="w-3.5 h-3.5" />
								<span>Broadcast</span>
							</button>
						</div>
					</div>
				</div>

				<!-- Paired Ring Devices & Live History Grid -->
				<div class="grid grid-cols-1 md:grid-cols-3 gap-6">
					<!-- Paired Devices Column -->
					<div class="md:col-span-1 space-y-3">
						<h3 class="text-xs font-bold uppercase tracking-wider text-slate-400 flex items-center gap-1.5">
							<Laptop class="w-3.5 h-3.5" />
							<span>Paired Devices ({appState.clipboardState?.devices?.length || 1})</span>
						</h3>

						<div class="space-y-2">
							{#if appState.clipboardState?.devices && appState.clipboardState.devices.length > 0}
								{#each appState.clipboardState.devices as dev}
									<div class="p-3 rounded-xl bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-800 flex items-center justify-between">
										<div class="flex items-center gap-2.5 min-w-0">
											<div class="p-2 rounded-lg bg-slate-100 dark:bg-slate-800 text-slate-600 dark:text-slate-300">
												{#if dev.deviceType === 'Mobile' || dev.deviceType === 'mobile'}
													<Smartphone class="w-4 h-4" />
												{:else if dev.deviceType === 'Server' || dev.deviceType === 'server'}
													<Server class="w-4 h-4" />
												{:else}
													<Laptop class="w-4 h-4" />
												{/if}
											</div>
											<div class="min-w-0">
												<p class="text-xs font-bold text-slate-800 dark:text-slate-200 truncate">{dev.alias}</p>
												<span class="text-[10px] text-slate-400 font-mono">{shortenFingerprint(dev.fingerprint)}</span>
											</div>
										</div>
										<span class="w-2 h-2 rounded-full bg-emerald-500" title="Online in Ring"></span>
									</div>
								{/each}
							{:else}
								<div class="p-3 rounded-xl bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-800 text-xs text-slate-400">
									Local Device Paired
								</div>
							{/if}
						</div>
					</div>

					<!-- Clipboard Stream History Column -->
					<div class="md:col-span-2 space-y-3">
						<h3 class="text-xs font-bold uppercase tracking-wider text-slate-400 flex items-center gap-1.5">
							<Clipboard class="w-3.5 h-3.5" />
							<span>Synchronized Clipboard Stream</span>
						</h3>

						<div class="space-y-2">
							{#if appState.clipboardHistory.length === 0}
								<div class="p-8 rounded-2xl bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-800 text-center text-slate-400 text-xs space-y-2">
									<Clipboard class="w-8 h-8 mx-auto opacity-30" />
									<p class="font-medium">No synchronized clipboard items yet</p>
									<p class="text-[11px] text-slate-500">Copy text or files on any paired device to sync instantly.</p>
								</div>
							{:else}
								{#each appState.clipboardHistory as item, idx}
									<div class="p-3.5 rounded-xl bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-800 flex items-start justify-between gap-3 shadow-xs">
										<div class="space-y-1 min-w-0 flex-1">
											{#if item.type === 'text'}
												<p class="text-xs font-mono text-slate-800 dark:text-slate-200 whitespace-pre-wrap break-all">
													{item.text}
												</p>
											{:else if item.type === 'image'}
												<div class="flex items-center gap-2">
													<div class="w-12 h-12 rounded bg-slate-100 dark:bg-slate-800 flex items-center justify-center text-xs text-slate-500">
														IMG
													</div>
													<span class="text-xs text-slate-500 font-mono">{item.mimeType} (Base64 Encrypted)</span>
												</div>
											{:else if item.type === 'files'}
												<div class="space-y-0.5">
													<span class="text-[10px] uppercase font-bold text-indigo-500">Files ({item.files.length})</span>
													{#each item.files as f}
														<p class="text-xs text-slate-700 dark:text-slate-300 truncate font-mono">{f}</p>
													{/each}
												</div>
											{/if}
										</div>

										<!-- Copy Action -->
										{#if item.type === 'text'}
											<button
												onclick={() => copyToClipboard(item.text, idx)}
												class="p-2 rounded-lg bg-slate-100 dark:bg-slate-800 hover:bg-slate-200 dark:hover:bg-slate-700 text-slate-600 dark:text-slate-300 transition-colors flex-none"
												title="Copy to local clipboard"
											>
												{#if copiedIndex === idx}
													<Check class="w-4 h-4 text-emerald-500" />
												{:else}
													<Copy class="w-4 h-4" />
												{/if}
											</button>
										{/if}
									</div>
								{/each}
							{/if}
						</div>
					</div>
				</div>
			</div>
		{:else if activeSubTab === 'search'}
			<!-- ============================================================== -->
			<!-- TAB 2: Federated File Search -->
			<!-- ============================================================== -->
			<div class="max-w-4xl mx-auto space-y-6">
				<!-- Search Header & Controls -->
				<div class="p-5 rounded-2xl bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-800 shadow-xs space-y-4">
					<div class="space-y-1">
						<h3 class="font-bold text-sm text-slate-900 dark:text-white flex items-center gap-2">
							<Search class="w-4 h-4 text-indigo-600" />
							<span>Federated Inverted Index Search</span>
						</h3>
						<p class="text-xs text-slate-500">
							Queries token and trigram inverted indexes across local Dufs shares and 1-hop / multi-hop overlay mesh peers in parallel.
						</p>
					</div>

					<div class="flex flex-col sm:flex-row gap-2">
						<div class="relative flex-1">
							<Search class="w-4 h-4 text-slate-400 absolute left-3 top-1/2 -translate-y-1/2" />
							<input
								type="text"
								bind:value={searchQuery}
								onkeydown={handleSearchKeydown}
								placeholder="Search filenames, documents, photos, or archives across LAN..."
								class="w-full pl-9 pr-3 py-2 text-xs sm:text-sm rounded-xl bg-slate-100 dark:bg-slate-800 border border-slate-200 dark:border-slate-700 text-slate-900 dark:text-white placeholder-slate-400 focus:outline-hidden focus:border-indigo-500"
							/>
						</div>

						<div class="relative sm:w-36">
							<Lock class="w-3.5 h-3.5 text-slate-400 absolute left-3 top-1/2 -translate-y-1/2" />
							<input
								type="password"
								bind:value={searchPin}
								placeholder="PIN (Optional)"
								class="w-full pl-8 pr-3 py-2 text-xs rounded-xl bg-slate-100 dark:bg-slate-800 border border-slate-200 dark:border-slate-700 text-slate-900 dark:text-white placeholder-slate-400 focus:outline-hidden"
							/>
						</div>

						<button
							onclick={handleSearch}
							disabled={!searchQuery.trim() || appState.isSearchingFiles}
							class="px-5 py-2 text-xs font-bold rounded-xl bg-indigo-600 hover:bg-indigo-700 disabled:opacity-40 text-white transition-colors flex items-center justify-center gap-1.5"
						>
							{#if appState.isSearchingFiles}
								<RefreshCw class="w-3.5 h-3.5 animate-spin" />
								<span>Searching...</span>
							{:else}
								<Search class="w-3.5 h-3.5" />
								<span>Search</span>
							{/if}
						</button>
					</div>

					<div class="flex items-center gap-3 text-[11px] text-slate-400 pt-2 border-t border-slate-100 dark:border-slate-800">
						<span class="flex items-center gap-1">
							<ShieldCheck class="w-3 h-3 text-emerald-500" />
							Hidden files (`.git`, `.part`) excluded
						</span>
						<span class="flex items-center gap-1">
							<Lock class="w-3 h-3 text-amber-500" />
							PIN-protected shares require matching PIN
						</span>
					</div>
				</div>

				<!-- Search Results List -->
				<div class="space-y-3">
					<div class="flex items-center justify-between">
						<h3 class="text-xs font-bold uppercase tracking-wider text-slate-400">
							Search Results ({appState.searchResults.length})
						</h3>
					</div>

					{#if appState.isSearchingFiles}
						<div class="p-8 text-center text-slate-400 text-xs">
							<RefreshCw class="w-6 h-6 mx-auto animate-spin text-indigo-500 mb-2" />
							<p>Aggregating hits from local catalog & mesh nodes...</p>
						</div>
					{:else if appState.searchResults.length === 0}
						<div class="p-8 rounded-2xl bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-800 text-center text-slate-400 text-xs space-y-1">
							{#if hasSearched}
								<Search class="w-6 h-6 mx-auto opacity-30 mb-1" />
								<p class="font-medium text-slate-700 dark:text-slate-300">No matching files found</p>
								<p class="text-[11px]">Try a broader search term or enter a folder PIN if searching private folders.</p>
							{:else}
								<Search class="w-6 h-6 mx-auto opacity-30 mb-1" />
								<p class="font-medium text-slate-700 dark:text-slate-300">Type a query above to search</p>
								<p class="text-[11px]">Federated search scans shared Dufs directories across all connected devices.</p>
							{/if}
						</div>
					{:else}
						<div class="space-y-2">
							{#each appState.searchResults as hit}
								<div class="p-4 rounded-xl bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-800 flex items-center justify-between gap-4 shadow-xs hover:border-indigo-300 dark:hover:border-indigo-800 transition-colors">
									<div class="flex items-start gap-3 min-w-0 flex-1">
										<div class="p-2.5 rounded-xl bg-indigo-50 dark:bg-indigo-950/60 text-indigo-600 dark:text-indigo-400 flex-none">
											<FileCode class="w-5 h-5" />
										</div>
										<div class="min-w-0 flex-1">
											<div class="flex items-center gap-2 mb-0.5">
												<h4 class="font-bold text-xs sm:text-sm text-slate-900 dark:text-white truncate">
													{hit.fileName}
												</h4>
												<span class="text-[10px] px-1.5 py-0.2 rounded-full font-mono bg-indigo-100 dark:bg-indigo-950 text-indigo-700 dark:text-indigo-300 font-bold">
													{hit.relevanceScore}% match
												</span>
												{#if hit.requiresPin}
													<span class="text-[10px] px-1.5 py-0.2 rounded bg-amber-50 dark:bg-amber-950 text-amber-600 flex items-center gap-0.5">
														<Lock class="w-2.5 h-2.5" /> PIN
													</span>
												{/if}
											</div>
											<p class="text-xs text-slate-500 dark:text-slate-400 font-mono truncate">
												{hit.relativePath}
											</p>
											<div class="flex items-center gap-3 text-[10px] text-slate-400 mt-1">
												<span>Size: {formatBytes(hit.sizeBytes)}</span>
												<span>Peer: {hit.peerAlias}</span>
												<span class="font-mono">{shortenFingerprint(hit.peerFp)}</span>
											</div>
										</div>
									</div>

									<!-- Action: Download / Open Link -->
									<a
										href={hit.downloadUrl}
										target="_blank"
										rel="noopener noreferrer"
										class="px-3 py-1.5 rounded-lg bg-indigo-50 dark:bg-indigo-950/60 hover:bg-indigo-100 text-indigo-600 dark:text-indigo-400 text-xs font-semibold flex items-center gap-1.5 transition-colors flex-none"
									>
										<Download class="w-3.5 h-3.5" />
										<span class="hidden sm:inline">Download</span>
									</a>
								</div>
							{/each}
						</div>
					{/if}
				</div>
			</div>
		{:else if activeSubTab === 'scratchpad'}
			<!-- ============================================================== -->
			<!-- TAB 3: Collaborative CRDT Scratchpad -->
			<!-- ============================================================== -->
			<div class="max-w-4xl mx-auto space-y-4">
				<!-- Scratchpad Header Card -->
				<div class="p-4 rounded-2xl bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-800 shadow-xs flex flex-col sm:flex-row sm:items-center justify-between gap-3">
					<div class="space-y-1">
						<div class="flex items-center gap-2">
							<FileCode class="w-5 h-5 text-indigo-600" />
							<h3 class="font-bold text-sm text-slate-900 dark:text-white">
								{scratchpadTitle}
							</h3>
						</div>
						<div class="flex items-center gap-3 text-[11px] text-slate-400">
							<span class="inline-flex items-center gap-1 font-mono">
								<Layers class="w-3 h-3 text-indigo-400" />
								RGA Sequence CRDT (SEC)
							</span>
							<span class="font-mono">Lamport: L={lamportClock}</span>
							<span class="font-mono">Atoms: {appState.collabDoc?.atoms?.length || 0}</span>
						</div>
					</div>

					<div class="flex items-center gap-2">
						<button
							onclick={handleCopyDocText}
							class="px-3 py-1.5 text-xs font-medium rounded-lg bg-slate-100 dark:bg-slate-800 hover:bg-slate-200 text-slate-700 dark:text-slate-200 transition-colors flex items-center gap-1"
							title="Copy Document Text"
						>
							{#if copiedDocSuccess}
								<Check class="w-3.5 h-3.5 text-emerald-500" />
								<span>Copied</span>
							{:else}
								<Copy class="w-3.5 h-3.5" />
								<span>Copy</span>
							{/if}
						</button>

						<button
							onclick={loadScratchpad}
							disabled={isSyncingDoc}
							class="px-3 py-1.5 text-xs font-semibold rounded-lg bg-indigo-50 dark:bg-indigo-950/60 text-indigo-600 dark:text-indigo-400 hover:bg-indigo-100 transition-colors flex items-center gap-1"
							title="Synchronize Scratchpad Snapshot"
						>
							<RefreshCw class="w-3.5 h-3.5 {isSyncingDoc ? 'animate-spin' : ''}" />
							<span>Sync</span>
						</button>
					</div>
				</div>

				<!-- Live Collaborative Editor Area -->
				<div class="relative rounded-2xl overflow-hidden border border-slate-200 dark:border-slate-800 bg-white dark:bg-slate-900 shadow-xs">
					<div class="p-2 bg-slate-50 dark:bg-slate-800/60 border-b border-slate-200 dark:border-slate-800 text-[11px] text-slate-400 flex items-center justify-between">
						<span>Markdown / Plain Text Editor</span>
						<span class="font-mono">Characters: {localScratchpadText.length}</span>
					</div>

					<textarea
						rows="18"
						value={localScratchpadText}
						oninput={handleScratchpadInput}
						placeholder="Type collaborative markdown notes, code snippets, or task lists here... All changes synchronize with Strong Eventual Consistency across local peers."
						class="w-full p-4 text-xs sm:text-sm font-mono bg-transparent text-slate-900 dark:text-slate-100 placeholder-slate-400 focus:outline-hidden resize-none leading-relaxed"
					></textarea>
				</div>
			</div>
		{/if}
	</div>
</div>
