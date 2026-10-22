<script lang="ts">
	import { appState } from '$lib/state.svelte';
	import { formatBytes, extractPasswordFromUrl } from '$lib/format';
	import { isTauri, isAndroid, updateConfig, regenerateShareLinkPin } from '$lib/api';
	import { open } from '@tauri-apps/plugin-dialog';
	import DirectoryPickerModal from '$lib/components/DirectoryPickerModal.svelte';
	import {
		ArrowLeft,
		Copy,
		QrCode,
		ExternalLink,
		Check,
		FileText,
		Folder,
		FolderPlus,
		FilePlus,
		MessageSquare,
		ClipboardPaste,
		X,
		ShieldCheck,
		Radio,
		AlertCircle,
		Key,
		RefreshCw,
		Eye,
		EyeOff
	} from 'lucide-svelte';

	let copiedUrl = $state<string | null>(null);
	let activeQrUrl = $state<string | null>(null);
	let encryptionEnabled = $state(true);
	let autoAccept = $state(appState.status?.shareLinkAutoAccept ?? false);
	let requirePin = $state(appState.status?.shareLinkRequirePin ?? true);
	let pin = $state(appState.status?.shareLinkPin ?? '');
	let isRegeneratingPin = $state(false);
	let copiedPin = $state(false);
	let showPin = $state(true);
	let showDirectoryPickerModal = $state(false);
	let saveTimeout: ReturnType<typeof setTimeout> | null = null;

	function onPinInput(val: string) {
		const extracted = extractPasswordFromUrl(val);
		pin = extracted;
		if (saveTimeout) clearTimeout(saveTimeout);
		saveTimeout = setTimeout(async () => {
			try {
				await updateConfig({ shareLinkPin: pin });
				if (appState.status) {
					appState.status.shareLinkPin = pin;
				}
			} catch (e) {
				console.error('Failed to update shareLinkPin:', e);
			}
		}, 300);
	}

	async function onPinBlur() {
		if (saveTimeout) clearTimeout(saveTimeout);
		const extracted = extractPasswordFromUrl(pin);
		pin = extracted;
		try {
			await updateConfig({ shareLinkPin: pin });
			if (appState.status) {
				appState.status.shareLinkPin = pin;
			}
		} catch (e) {
			console.error('Failed to update shareLinkPin:', e);
		}
	}

	function onPinPaste(e: ClipboardEvent) {
		const pastedText = e.clipboardData?.getData('text') ?? '';
		if (pastedText) {
			e.preventDefault();
			const extracted = extractPasswordFromUrl(pastedText);
			pin = extracted;
			onPinBlur();
		}
	}

	// Derived list of valid interface URLs (including Basic Auth credentials when PIN is required)
	let downloadUrls = $derived.by(() => {
		const port = appState.status?.port || 53317;
		const proto = encryptionEnabled ? 'https' : 'http';
		const ifaces = appState.status?.interfaces || [];
		const authPrefix = requirePin && pin ? `:${pin}@` : '';

		// Filter to IPv4 addresses first for clean browser URLs
		const v4 = ifaces.filter((i) => !i.isIpv6 && i.address && i.address !== '127.0.0.1');
		if (v4.length === 0) {
			return [`${proto}://${authPrefix}127.0.0.1:${port}`];
		}
		return v4.map((i) => `${proto}://${authPrefix}${i.address}:${port}`);
	});

	async function handleAutoAcceptChange() {
		try {
			await updateConfig({ shareLinkAutoAccept: autoAccept });
			if (appState.status) {
				appState.status.shareLinkAutoAccept = autoAccept;
			}
		} catch (e) {
			console.error('Failed to update shareLinkAutoAccept:', e);
		}
	}

	async function handleRequirePinChange() {
		try {
			await updateConfig({ shareLinkRequirePin: requirePin });
			if (appState.status) {
				appState.status.shareLinkRequirePin = requirePin;
			}
		} catch (e) {
			console.error('Failed to update shareLinkRequirePin:', e);
		}
	}

	async function handleRegeneratePin() {
		isRegeneratingPin = true;
		try {
			const newPin = await regenerateShareLinkPin();
			pin = newPin;
			if (appState.status) {
				appState.status.shareLinkPin = newPin;
			}
		} catch (e) {
			console.error('Failed to regenerate PIN:', e);
		} finally {
			setTimeout(() => {
				isRegeneratingPin = false;
			}, 350);
		}
	}

	async function copyPinToClipboard() {
		if (!pin) return;
		try {
			await navigator.clipboard.writeText(pin);
			copiedPin = true;
			setTimeout(() => {
				copiedPin = false;
			}, 2000);
		} catch (e) {
			console.error('Failed to copy PIN:', e);
		}
	}

	async function copyToClipboard(url: string) {
		try {
			await navigator.clipboard.writeText(url);
			copiedUrl = url;
			setTimeout(() => {
				if (copiedUrl === url) copiedUrl = null;
			}, 2000);
		} catch (e) {
			console.error('Failed to copy URL:', e);
		}
	}

	function openInBrowser(url: string) {
		if (typeof window !== 'undefined') {
			window.open(url, '_blank');
		}
	}

	async function pickFiles() {
		if (isTauri()) {
			try {
				const res = await open({ multiple: true, directory: false });
				if (res) {
					const paths = Array.isArray(res) ? res : [res];
					appState.addFiles(paths);
					await appState.startShareViaLink();
				}
			} catch (e) {
				console.error('Pick files error:', e);
			}
		}
	}

	async function pickFolder() {
		if (isAndroid()) {
			showDirectoryPickerModal = true;
			return;
		}
		if (isTauri()) {
			try {
				const res = await open({ directory: true, multiple: false });
				if (res && typeof res === 'string') {
					await onFolderSelected(res);
				}
			} catch (e) {
				console.warn('Pick folder error, falling back to modal:', e);
				showDirectoryPickerModal = true;
			}
		} else {
			showDirectoryPickerModal = true;
		}
	}

	async function onFolderSelected(path: string) {
		appState.addFolder(path);
		await appState.startShareViaLink();
	}

	async function pasteClipboard() {
		const success = await appState.pasteFromClipboard();
		if (success) {
			await appState.startShareViaLink();
		}
	}

	async function handleRemoveItem(idx: number) {
		appState.removePreparedItem(idx);
		if (appState.preparedItems.length === 0) {
			await appState.stopShareViaLink();
		} else {
			await appState.startShareViaLink();
		}
	}
</script>

<div class="flex flex-col h-full overflow-y-auto p-4 sm:p-6 space-y-5">
	<!-- Top Bar -->
	<div class="flex items-center justify-between border-b border-slate-200 dark:border-slate-800 pb-3">
		<div class="flex items-center gap-3">
			<button
				onclick={() => {
					appState.sendMode = 'single';
				}}
				class="p-1.5 rounded-lg text-slate-500 hover:text-slate-900 dark:text-slate-400 dark:hover:text-white hover:bg-slate-100 dark:hover:bg-slate-800 transition-colors cursor-pointer"
				title="Back to recipient selection"
			>
				<ArrowLeft class="w-5 h-5" />
			</button>
			<div>
				<h2 class="text-lg sm:text-xl font-bold text-slate-900 dark:text-slate-100">Share via link</h2>
				<p class="text-xs text-slate-500 dark:text-slate-400">
					Recipients without LanFlare/LocalSend can download files in any web browser
				</p>
			</div>
		</div>

		<div class="flex items-center gap-2">
			{#if appState.isSharingLink}
				<span class="flex items-center gap-1.5 text-xs font-semibold px-2.5 py-1 rounded-full bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border border-emerald-500/20">
					<span class="w-2 h-2 rounded-full bg-emerald-500 animate-pulse"></span>
					<span>Active Server</span>
				</span>
			{/if}
		</div>
	</div>

	<!-- Links Card Box (Matching LocalSend layout) -->
	<div class="space-y-2">
		<h3 class="text-xs font-semibold text-slate-700 dark:text-slate-300">
			Open one of these links in your browser:
		</h3>
		<div class="bg-indigo-50/70 dark:bg-indigo-950/30 border border-indigo-200/80 dark:border-indigo-800/40 rounded-2xl p-4 space-y-2.5 shadow-sm">
			{#each downloadUrls as url}
				<div class="flex items-center justify-between gap-3 text-xs font-mono py-1 px-1.5 rounded-lg hover:bg-indigo-100/50 dark:hover:bg-indigo-900/30 transition-colors">
					<span class="text-indigo-950 dark:text-indigo-200 truncate select-all">{url}</span>
					<div class="flex items-center gap-1.5 flex-shrink-0">
						<!-- Copy button -->
						<button
							onclick={() => copyToClipboard(url)}
							class="p-1.5 rounded-md hover:bg-indigo-200/60 dark:hover:bg-indigo-800/60 text-slate-600 dark:text-slate-300 transition-colors cursor-pointer"
							title="Copy link"
						>
							{#if copiedUrl === url}
								<Check class="w-4 h-4 text-emerald-600 dark:text-emerald-400" />
							{:else}
								<Copy class="w-4 h-4" />
							{/if}
						</button>

						<!-- QR code button -->
						<button
							onclick={() => (activeQrUrl = url)}
							class="p-1.5 rounded-md hover:bg-indigo-200/60 dark:hover:bg-indigo-800/60 text-slate-600 dark:text-slate-300 transition-colors cursor-pointer"
							title="Show QR Code"
						>
							<QrCode class="w-4 h-4" />
						</button>

						<!-- Open in browser button -->
						<button
							onclick={() => openInBrowser(url)}
							class="p-1.5 rounded-md hover:bg-indigo-200/60 dark:hover:bg-indigo-800/60 text-slate-600 dark:text-slate-300 transition-colors cursor-pointer"
							title="Open in default browser"
						>
							<ExternalLink class="w-4 h-4" />
						</button>
					</div>
				</div>
			{/each}
		</div>
	</div>

	<!-- Requests Section (Matching LocalSend layout) -->
	{#if appState.pendingBrowserDownloads.length > 0}
		<div class="space-y-2">
			<div class="flex items-center justify-between">
				<h3 class="text-xs font-semibold text-slate-700 dark:text-slate-300">
					Requests ({appState.pendingBrowserDownloads.length})
				</h3>
				<span class="text-[11px] text-amber-600 dark:text-amber-400 font-medium">
					Click to approve or decline download
				</span>
			</div>
			<div class="space-y-2">
				{#each appState.pendingBrowserDownloads as req (req.downloadId)}
					<div
						class="flex items-center justify-between p-3.5 bg-amber-500/10 border border-amber-500/30 rounded-xl shadow-xs"
					>
						<div class="truncate max-w-[75%]">
							<div class="text-xs font-semibold text-slate-900 dark:text-slate-100 truncate" title={req.fileName}>
								{req.fileName}
							</div>
							<div class="text-[11px] text-slate-500 dark:text-slate-400 font-mono mt-0.5">
								{req.remoteIp} {req.fileSize > 0 ? `• ${formatBytes(req.fileSize)}` : ''}
							</div>
						</div>
						<div class="flex items-center gap-2 flex-shrink-0">
							<button
								onclick={() => appState.respondBrowserDownload(req.downloadId, false)}
								class="p-1.5 rounded-lg text-rose-600 dark:text-rose-400 hover:bg-rose-100 dark:hover:bg-rose-950/50 transition-colors cursor-pointer"
								title="Decline download request"
							>
								<X class="w-5 h-5" />
							</button>
							<button
								onclick={() => appState.respondBrowserDownload(req.downloadId, true)}
								class="p-1.5 rounded-lg text-emerald-600 dark:text-emerald-400 hover:bg-emerald-100 dark:hover:bg-emerald-950/50 transition-colors cursor-pointer"
								title="Accept and start download"
							>
								<Check class="w-5 h-5" />
							</button>
						</div>
					</div>
				{/each}
			</div>
		</div>
	{/if}

	<!-- Shared Content Basket -->
	<div class="space-y-3">
		<div class="flex items-center justify-between">
			<h3 class="text-xs font-semibold text-slate-700 dark:text-slate-300 flex items-center gap-2">
				<span>Shared Files ({appState.preparedItems.length})</span>
				{#if appState.totalPreparedSize > 0}
					<span class="font-mono text-slate-500 font-normal">({formatBytes(appState.totalPreparedSize)})</span>
				{/if}
			</h3>

			<!-- Quick Add Action Buttons -->
			<div class="flex items-center gap-2">
				<button
					onclick={pickFiles}
					class="flex items-center gap-1 py-1 px-2.5 rounded-lg bg-slate-100 dark:bg-slate-800 hover:bg-slate-200 dark:hover:bg-slate-700 text-slate-700 dark:text-slate-300 text-xs font-medium transition-colors cursor-pointer"
				>
					<FilePlus class="w-3.5 h-3.5" />
					<span>Files</span>
				</button>
				{#if !isAndroid()}
					<button
						onclick={pickFolder}
						class="flex items-center gap-1 py-1 px-2.5 rounded-lg bg-slate-100 dark:bg-slate-800 hover:bg-slate-200 dark:hover:bg-slate-700 text-slate-700 dark:text-slate-300 text-xs font-medium transition-colors cursor-pointer"
					>
						<FolderPlus class="w-3.5 h-3.5" />
						<span>Folder</span>
					</button>
				{/if}
				<button
					onclick={() => (appState.showTextModal = true)}
					class="flex items-center gap-1 py-1 px-2.5 rounded-lg bg-slate-100 dark:bg-slate-800 hover:bg-slate-200 dark:hover:bg-slate-700 text-slate-700 dark:text-slate-300 text-xs font-medium transition-colors cursor-pointer"
				>
					<MessageSquare class="w-3.5 h-3.5" />
					<span>Text</span>
				</button>
				<button
					onclick={pasteClipboard}
					class="flex items-center gap-1 py-1 px-2.5 rounded-lg bg-slate-100 dark:bg-slate-800 hover:bg-slate-200 dark:hover:bg-slate-700 text-slate-700 dark:text-slate-300 text-xs font-medium transition-colors cursor-pointer"
				>
					<ClipboardPaste class="w-3.5 h-3.5" />
					<span>Paste</span>
				</button>
			</div>
		</div>

		{#if appState.preparedItems.length === 0}
			<div class="border-2 border-dashed border-slate-300 dark:border-slate-800 rounded-xl p-8 flex flex-col items-center justify-center gap-2 text-center text-slate-400">
				<FileText class="w-8 h-8 text-slate-400" />
				<p class="text-xs font-medium text-slate-600 dark:text-slate-400">No items selected yet</p>
				<p class="text-[11px] text-slate-400">Use the buttons above to add files, folders, or text to share via link</p>
			</div>
		{:else}
			<div class="bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-800 rounded-xl divide-y divide-slate-100 dark:divide-slate-800 max-h-56 overflow-y-auto">
				{#each appState.preparedItems as item, idx}
					<div class="flex items-center justify-between p-3 text-xs">
						<div class="flex items-center gap-2.5 truncate max-w-[85%]">
							{#if item.type === 'folder'}
								<Folder class="w-4 h-4 text-amber-500 flex-shrink-0" />
							{:else if item.type === 'text'}
								<MessageSquare class="w-4 h-4 text-violet-500 flex-shrink-0" />
							{:else}
								<FileText class="w-4 h-4 text-indigo-500 flex-shrink-0" />
							{/if}
							<div class="truncate">
								<span class="font-medium text-slate-900 dark:text-slate-200 block truncate">{item.name}</span>
								<span class="text-[11px] text-slate-400 font-mono block">
									{item.summary || item.type} {item.size > 0 ? `• ${formatBytes(item.size)}` : ''}
								</span>
							</div>
						</div>
						<button
							onclick={() => handleRemoveItem(idx)}
							class="p-1 rounded-lg text-slate-400 hover:text-rose-600 dark:hover:text-rose-400 hover:bg-slate-100 dark:hover:bg-slate-800 transition-colors"
						>
							<X class="w-4 h-4" />
						</button>
					</div>
				{/each}
			</div>
		{/if}
	</div>

	<!-- Options & Security Section (Matching Screenshot) -->
	<div class="space-y-3 pt-2">
		<div class="flex items-start gap-3">
			<input
				type="checkbox"
				id="encryption-chk"
				bind:checked={encryptionEnabled}
				class="mt-1 w-4 h-4 accent-indigo-600 rounded cursor-pointer"
			/>
			<div>
				<label for="encryption-chk" class="text-xs font-semibold text-slate-800 dark:text-slate-200 cursor-pointer">
					Encryption (HTTPS)
				</label>
				<p class="text-[11px] text-amber-600 dark:text-amber-400 mt-0.5">
					LanFlare uses a self-signed TLS certificate. You need to accept it in your browser when connecting.
				</p>
			</div>
		</div>

		<div class="flex items-start gap-3">
			<input
				type="checkbox"
				id="auto-accept-chk"
				bind:checked={autoAccept}
				onchange={handleAutoAcceptChange}
				class="mt-1 w-4 h-4 accent-indigo-600 rounded cursor-pointer"
			/>
			<div>
				<label for="auto-accept-chk" class="text-xs font-semibold text-slate-800 dark:text-slate-200 cursor-pointer">
					Automatically accept requests
				</label>
				<p class="text-[11px] text-slate-500 dark:text-slate-400 mt-0.5">
					{autoAccept
						? 'Manual approval required: each browser download request must be accepted'
						: 'Downloads proceed automatically without manual approval'}
				</p>
			</div>
		</div>

		<div class="flex items-start gap-3">
			<input
				type="checkbox"
				id="require-pin-chk"
				bind:checked={requirePin}
				onchange={handleRequirePinChange}
				class="mt-1 w-4 h-4 accent-indigo-600 rounded cursor-pointer"
			/>
			<div class="flex-1 space-y-1.5">
				<label for="require-pin-chk" class="text-xs font-semibold text-slate-800 dark:text-slate-200 cursor-pointer">
					Require PIN
				</label>
				<p class="text-[11px] text-slate-500 dark:text-slate-400">
					Protects the link with HTTP Basic Auth password. Included in the share link and QR code.
				</p>
				{#if requirePin}
					<div class="flex flex-wrap items-center gap-2 pt-1.5">
						<!-- Editable PIN / Password Input Field -->
						<div
							class="flex items-center gap-2 px-3 py-1.5 bg-white dark:bg-slate-900 border border-slate-300 dark:border-slate-700 focus-within:border-indigo-500 focus-within:ring-2 focus-within:ring-indigo-500/20 rounded-xl text-xs shadow-xs transition-all flex-1 min-w-[200px] max-w-sm"
						>
							<Key class="w-3.5 h-3.5 text-indigo-500 flex-shrink-0" />
							<input
								type={showPin ? 'text' : 'password'}
								value={pin}
								oninput={(e) => onPinInput(e.currentTarget.value)}
								onblur={onPinBlur}
								onpaste={onPinPaste}
								placeholder="Enter password or paste link"
								class="bg-transparent border-none outline-none font-mono font-medium text-slate-800 dark:text-slate-100 placeholder:text-slate-400 text-xs w-full"
								title="Custom password / PIN. Pasting full share links will automatically extract the password."
							/>
							<button
								type="button"
								onclick={() => (showPin = !showPin)}
								class="p-1 rounded text-slate-400 hover:text-slate-600 dark:hover:text-slate-200 transition-colors cursor-pointer flex-shrink-0"
								title={showPin ? 'Hide password' : 'Show password'}
							>
								{#if showPin}
									<EyeOff class="w-3.5 h-3.5" />
								{:else}
									<Eye class="w-3.5 h-3.5" />
								{/if}
							</button>
							<button
								type="button"
								onclick={copyPinToClipboard}
								class="p-1 rounded text-slate-400 hover:text-indigo-600 dark:hover:text-indigo-300 transition-colors cursor-pointer flex-shrink-0"
								title="Copy password"
							>
								{#if copiedPin}
									<Check class="w-3.5 h-3.5 text-emerald-500" />
								{:else}
									<Copy class="w-3.5 h-3.5" />
								{/if}
							</button>
						</div>

						<button
							type="button"
							onclick={handleRegeneratePin}
							class="flex items-center gap-1.5 px-3 py-2 rounded-xl bg-slate-100 dark:bg-slate-800 hover:bg-slate-200 dark:hover:bg-slate-700 text-slate-700 dark:text-slate-300 text-xs font-medium transition-colors cursor-pointer border border-slate-200 dark:border-slate-700/60 shadow-xs flex-shrink-0"
							title="Regenerate random PIN"
						>
							<RefreshCw class="w-3.5 h-3.5 {isRegeneratingPin ? 'animate-spin' : ''}" />
							<span>Regenerate PIN</span>
						</button>
					</div>
				{/if}
			</div>
		</div>
	</div>
</div>

<!-- QR Code Modal -->
{#if activeQrUrl}
	<div
		class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 dark:bg-black/75 backdrop-blur-sm p-4 animate-in fade-in duration-150"
	>
		<div
			class="w-full max-w-sm bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-800 rounded-2xl shadow-2xl p-6 space-y-4 animate-in zoom-in-95 duration-150 text-center"
		>
			<div class="flex items-center justify-between border-b border-slate-200 dark:border-slate-800 pb-3">
				<h3 class="text-sm font-semibold text-slate-900 dark:text-slate-100">Scan QR to Download</h3>
				<button
					onclick={() => (activeQrUrl = null)}
					class="p-1 rounded-lg text-slate-400 hover:text-slate-700 dark:hover:text-slate-200 hover:bg-slate-100 dark:hover:bg-slate-800 transition-colors"
				>
					<X class="w-4 h-4" />
				</button>
			</div>

			<div class="flex flex-col items-center justify-center p-4 bg-white rounded-xl shadow-inner border border-slate-200">
				<img
					src={`https://api.qrserver.com/v1/create-qr-code/?size=220x220&data=${encodeURIComponent(activeQrUrl)}`}
					alt="QR Code"
					class="w-52 h-52 object-contain"
				/>
			</div>

			<p class="text-xs font-mono text-slate-500 truncate max-w-full" title={activeQrUrl}>
				{activeQrUrl}
			</p>

			<button
				onclick={() => {
					if (activeQrUrl) copyToClipboard(activeQrUrl);
				}}
				class="w-full flex items-center justify-center gap-2 py-2 px-4 rounded-xl bg-indigo-600 hover:bg-indigo-500 text-white text-xs font-medium shadow-md shadow-indigo-600/20 transition-all cursor-pointer"
			>
				<Copy class="w-4 h-4" />
				<span>{copiedUrl === activeQrUrl ? 'Copied Link!' : 'Copy Link'}</span>
			</button>
		</div>
	</div>
{/if}

<DirectoryPickerModal
	isOpen={showDirectoryPickerModal}
	onClose={() => (showDirectoryPickerModal = false)}
	onSelect={onFolderSelected}
/>
