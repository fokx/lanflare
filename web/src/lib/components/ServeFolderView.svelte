<script lang="ts">
	import { onMount } from 'svelte';
	import { appState } from '$lib/state.svelte';
	import { formatBytes, extractPasswordFromUrl } from '$lib/format';
	import {
		isTauri,
		isAndroid,
		startDufsShare,
		stopDufsShare,
		getDufsShareStatus,
		listDirectory
	} from '$lib/api';
	import type { DufsShareStatusDto, DirEntryDto } from '$lib/types';
	import { open } from '@tauri-apps/plugin-dialog';
	import DirectoryPickerModal from './DirectoryPickerModal.svelte';
	import {
		ArrowLeft,
		Folder,
		FolderPlus,
		Copy,
		QrCode,
		ExternalLink,
		Check,
		RefreshCw,
		ShieldCheck,
		Key,
		UploadCloud,
		Trash2,
		Radio,
		Eye,
		EyeOff,
		FileText,
		X,
		HardDrive,
		Power
	} from 'lucide-svelte';

	let status = $state<DufsShareStatusDto | null>(null);
	let selectedFolderPath = $state<string>('');
	let selectedFolderName = $state<string>('');
	let isServerRunning = $state<boolean>(false);
	let allowUpload = $state<boolean>(false);
	let allowDelete = $state<boolean>(false);
	let requirePin = $state<boolean>(true);
	let pin = $state<string>(appState.status?.shareLinkPin || 'k8mP2x9L4w');
	let isStarting = $state<boolean>(false);
	let copiedUrl = $state<string | null>(null);
	let activeQrUrl = $state<string | null>(null);
	let showPin = $state<boolean>(true);
	let showFolderPickerModal = $state<boolean>(false);

	// Folder content preview
	let folderEntries = $state<DirEntryDto[]>([]);
	let isLoadingPreview = $state<boolean>(false);

	onMount(async () => {
		await loadStatus();
	});

	async function loadStatus() {
		try {
			const s = await getDufsShareStatus();
			if (s && s.isActive) {
				status = s;
				selectedFolderPath = s.folderPath;
				selectedFolderName = s.folderName;
				isServerRunning = true;
				allowUpload = s.allowUpload;
				allowDelete = s.allowDelete;
				requirePin = !!s.authPin;
				if (s.authPin) pin = s.authPin;
				loadPreview(s.folderPath);
			} else {
				// Default initial folder
				if (isAndroid()) {
					selectedFolderPath = '/storage/emulated/0/Download';
					selectedFolderName = 'Download';
				} else if (appState.status?.downloadDir) {
					selectedFolderPath = appState.status.downloadDir;
					selectedFolderName = selectedFolderPath.split('/').pop() || 'Downloads';
				}
				if (selectedFolderPath) {
					loadPreview(selectedFolderPath);
				}
			}
		} catch (e) {
			console.error('Failed to get dufs status:', e);
		}
	}

	async function loadPreview(path: string) {
		if (!path) return;
		isLoadingPreview = true;
		try {
			const res = await listDirectory(path);
			folderEntries = res.entries;
		} catch (e) {
			console.warn('Failed to load folder preview:', e);
		} finally {
			isLoadingPreview = false;
		}
	}

	async function handlePickFolder() {
		if (isAndroid()) {
			// On Android, open the dedicated DirectoryPickerModal
			showFolderPickerModal = true;
			return;
		}

		if (isTauri()) {
			try {
				const res = await open({ directory: true, multiple: false });
				if (res && typeof res === 'string') {
					onFolderSelected(res);
				}
			} catch (e) {
				console.warn('Native folder dialog failed, falling back to modal:', e);
				showFolderPickerModal = true;
			}
		} else {
			showFolderPickerModal = true;
		}
	}

	function onFolderSelected(path: string) {
		selectedFolderPath = path;
		const name = path.split('/').filter(Boolean).pop() || path;
		selectedFolderName = name;
		loadPreview(path);
		if (isServerRunning) {
			updateServerConfig();
		}
	}

	async function updateServerConfig() {
		if (!isServerRunning || !selectedFolderPath) return;
		try {
			const res = await startDufsShare({
				folderPath: selectedFolderPath,
				allowUpload,
				allowDelete,
				requirePin,
				pin: requirePin ? pin : undefined
			});
			status = res;
		} catch (e: any) {
			console.error('Failed to update folder server config:', e);
		}
	}

	async function toggleServer() {
		if (isServerRunning) {
			try {
				await stopDufsShare();
				isServerRunning = false;
				status = null;
			} catch (e) {
				console.error('Failed to stop dufs share:', e);
			}
		} else {
			if (!selectedFolderPath) {
				handlePickFolder();
				return;
			}
			isStarting = true;
			try {
				const res = await startDufsShare({
					folderPath: selectedFolderPath,
					allowUpload,
					allowDelete,
					requirePin,
					pin: requirePin ? pin : undefined
				});
				status = res;
				isServerRunning = true;
			} catch (e: any) {
				console.error('Failed to start dufs share:', e);
				alert(`Failed to start folder server: ${e?.message || e}`);
			} finally {
				isStarting = false;
			}
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

	function regeneratePin() {
		const chars = '23456789abcdefghjkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ';
		let res = '';
		for (let i = 0; i < 10; i++) {
			res += chars.charAt(Math.floor(Math.random() * chars.length));
		}
		pin = res;
		if (isServerRunning) {
			updateServerConfig();
		}
	}

	// Derived access URLs
	let accessUrls = $derived.by(() => {
		if (status && status.urls && status.urls.length > 0) {
			return status.urls;
		}
		const port = appState.status?.port || 53317;
		const proto = 'https';
		const ifaces = appState.status?.interfaces || [];
		const authPrefix = requirePin && pin ? `:${pin}@` : '';
		const v4 = ifaces.filter((i) => !i.isIpv6 && i.address && i.address !== '127.0.0.1');
		if (v4.length === 0) {
			return [`${proto}://${authPrefix}127.0.0.1:${port}/`];
		}
		return v4.map((i) => `${proto}://${authPrefix}${i.address}:${port}/`);
	});
</script>

<div class="flex flex-col h-full overflow-y-auto p-4 sm:p-6 space-y-5">
	<!-- Top Bar -->
	<div class="flex items-center justify-between border-b border-slate-200 dark:border-slate-800 pb-3">
		<div class="flex items-center gap-3">
			<button
				onclick={() => {
					appState.sendMode = 'single';
				}}
				class="p-2 rounded-xl border border-slate-200 dark:border-slate-800 hover:bg-slate-100 dark:hover:bg-slate-800 text-slate-600 dark:text-slate-300 transition-colors cursor-pointer"
				title="Back to Device Discovery"
			>
				<ArrowLeft class="w-4 h-4" />
			</button>
			<div>
				<h2 class="text-base font-bold text-slate-900 dark:text-white flex items-center gap-2 leading-none">
					Serve Folder
					<span class="text-[10px] font-mono px-2 py-0.5 rounded-full bg-amber-100 dark:bg-amber-900/40 text-amber-700 dark:text-amber-300 font-semibold border border-amber-300 dark:border-amber-700/50">
						Dufs Mode
					</span>
				</h2>
				<p class="text-xs text-slate-500 dark:text-slate-400 mt-1">
					Expose an entire folder to local peers with high-speed browsing, streaming & ZIP downloads.
				</p>
			</div>
		</div>

		<!-- Top Server Power Toggle -->
		<button
			onclick={toggleServer}
			disabled={isStarting}
			class="flex items-center gap-2 px-3.5 py-2 rounded-xl text-xs font-semibold shadow-md transition-all cursor-pointer {isServerRunning
				? 'bg-emerald-600 hover:bg-emerald-500 text-white shadow-emerald-600/20'
				: 'bg-indigo-600 hover:bg-indigo-500 text-white shadow-indigo-600/20'}"
		>
			<Power class="w-4 h-4" />
			<span>{isServerRunning ? 'Server Active' : 'Start Server'}</span>
		</button>
	</div>

	<!-- Folder Selection Card -->
	<div class="bg-white dark:bg-slate-900/90 border border-slate-200 dark:border-slate-800 rounded-2xl p-4 sm:p-5 shadow-xs space-y-3">
		<div class="flex items-center justify-between">
			<div class="flex items-center gap-2">
				<div class="p-2 rounded-xl bg-amber-500/10 text-amber-500">
					<Folder class="w-5 h-5 fill-amber-500/20 stroke-[2]" />
				</div>
				<div>
					<h3 class="text-sm font-bold text-slate-900 dark:text-white">Served Folder</h3>
					<p class="text-[11px] text-slate-500">
						Peers on the network will be able to browse all subdirectories and files inside.
					</p>
				</div>
			</div>

			<button
				onclick={handlePickFolder}
				class="flex items-center gap-1.5 px-3 py-1.5 rounded-xl border border-slate-200 dark:border-slate-700 hover:border-indigo-400 bg-slate-50 dark:bg-slate-800 hover:bg-indigo-50 dark:hover:bg-indigo-950/40 text-slate-700 dark:text-slate-300 hover:text-indigo-600 text-xs font-semibold transition-all cursor-pointer shadow-2xs"
			>
				<FolderPlus class="w-3.5 h-3.5 text-amber-500" />
				<span>{selectedFolderPath ? 'Change Folder' : 'Select Folder'}</span>
			</button>
		</div>

		{#if selectedFolderPath}
			<div class="p-3 bg-slate-50 dark:bg-slate-950/60 border border-slate-200 dark:border-slate-800/80 rounded-xl space-y-1.5">
				<div class="flex items-center justify-between">
					<span class="text-xs font-bold text-slate-800 dark:text-slate-200 truncate">
						{selectedFolderName}
					</span>
					{#if status}
						<span class="text-[11px] text-slate-500 font-mono">
							{status.fileCount} items ({formatBytes(status.totalSize)})
						</span>
					{/if}
				</div>
				<p class="text-[11px] font-mono text-slate-500 dark:text-slate-400 truncate" title={selectedFolderPath}>
					{selectedFolderPath}
				</p>
			</div>
		{:else}
			<button
				type="button"
				onclick={handlePickFolder}
				class="w-full p-6 border-2 border-dashed border-slate-200 dark:border-slate-800 hover:border-indigo-400 dark:hover:border-indigo-600 rounded-xl text-center cursor-pointer transition-colors space-y-1 bg-transparent"
			>
				<FolderPlus class="w-8 h-8 text-slate-400 mx-auto stroke-[1.5]" />
				<p class="text-xs font-semibold text-slate-700 dark:text-slate-300">Click to select folder to serve</p>
				<p class="text-[11px] text-slate-400">Works on all platforms, including Android internal storage</p>
			</button>
		{/if}
	</div>

	<!-- Live Server URLs Card (Visible when active) -->
	{#if isServerRunning}
		<div class="bg-gradient-to-br from-indigo-500/5 via-violet-500/5 to-transparent border border-indigo-200 dark:border-indigo-900/60 rounded-2xl p-4 sm:p-5 shadow-xs space-y-4">
			<div class="flex items-center justify-between">
				<div class="flex items-center gap-2.5">
					<span class="relative flex h-3 w-3">
						<span class="animate-ping absolute inline-flex h-full w-full rounded-full bg-emerald-400 opacity-75"></span>
						<span class="relative inline-flex rounded-full h-3 w-3 bg-emerald-500"></span>
					</span>
					<div>
						<h3 class="text-sm font-bold text-slate-900 dark:text-white flex items-center gap-1.5">
							Live Network Access URLs
						</h3>
						<p class="text-[11px] text-slate-500 dark:text-slate-400">
							Any device on your Wi-Fi or LAN can open these links in any browser.
						</p>
					</div>
				</div>

				<span class="text-[11px] font-mono font-semibold px-2 py-0.5 rounded-full bg-indigo-100 dark:bg-indigo-900/50 text-indigo-700 dark:text-indigo-300">
					Port {status?.port || 53317}
				</span>
			</div>

			<!-- URL list -->
			<div class="space-y-2">
				{#each accessUrls as url}
					<div class="flex items-center justify-between p-3 bg-white dark:bg-slate-900/90 border border-slate-200 dark:border-slate-800 rounded-xl gap-2 shadow-2xs hover:border-indigo-300 dark:hover:border-indigo-700 transition-colors">
						<span class="text-xs font-mono text-indigo-600 dark:text-indigo-400 font-semibold truncate select-all">
							{url}
						</span>

						<div class="flex items-center gap-1.5 flex-shrink-0">
							<button
								onclick={() => copyToClipboard(url)}
								class="p-1.5 rounded-lg border border-slate-200 dark:border-slate-700 hover:bg-slate-100 dark:hover:bg-slate-800 text-slate-600 dark:text-slate-300 transition-colors"
								title="Copy URL"
							>
								{#if copiedUrl === url}
									<Check class="w-3.5 h-3.5 text-emerald-500" />
								{:else}
									<Copy class="w-3.5 h-3.5" />
								{/if}
							</button>

							<button
								onclick={() => (activeQrUrl = url)}
								class="p-1.5 rounded-lg border border-slate-200 dark:border-slate-700 hover:bg-slate-100 dark:hover:bg-slate-800 text-slate-600 dark:text-slate-300 transition-colors"
								title="Show QR Code"
							>
								<QrCode class="w-3.5 h-3.5" />
							</button>

							<button
								onclick={() => openInBrowser(url)}
								class="p-1.5 rounded-lg border border-slate-200 dark:border-slate-700 hover:bg-slate-100 dark:hover:bg-slate-800 text-slate-600 dark:text-slate-300 transition-colors"
								title="Open in Browser"
							>
								<ExternalLink class="w-3.5 h-3.5" />
							</button>
						</div>
					</div>
				{/each}

				{#if requirePin}
					<div class="flex items-start gap-2.5 p-3 bg-amber-500/10 border border-amber-300 dark:border-amber-700/50 rounded-xl text-amber-800 dark:text-amber-300 text-xs">
						<Key class="w-4 h-4 mt-0.5 flex-shrink-0" />
						<div>
							<span class="font-bold">Browser Login Hint:</span>
							When opening the link, browsers will prompt for credentials.
							<span class="font-semibold underline">Leave Username empty</span> (not required) and enter only the <span class="font-semibold underline">Password / PIN</span>:
							<code class="font-mono bg-amber-200/50 dark:bg-amber-950/60 px-1.5 py-0.5 rounded font-bold ml-1">{pin}</code>
						</div>
					</div>
				{/if}
			</div>
		</div>
	{/if}

	<!-- Configuration & Permissions -->
	<div class="bg-white dark:bg-slate-900/90 border border-slate-200 dark:border-slate-800 rounded-2xl p-4 sm:p-5 shadow-xs space-y-4">
		<h3 class="text-sm font-bold text-slate-900 dark:text-white flex items-center gap-2">
			<ShieldCheck class="w-4 h-4 text-indigo-500" />
			<span>Access Control & Permissions</span>
		</h3>

		<div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
			<!-- Allow Peer Uploads -->
			<label class="flex items-start gap-3 p-3 rounded-xl border border-slate-200 dark:border-slate-800 bg-slate-50/50 dark:bg-slate-950/40 cursor-pointer hover:border-indigo-300 transition-colors">
				<input
					type="checkbox"
					bind:checked={allowUpload}
					onchange={() => {
						if (isServerRunning) updateServerConfig();
					}}
					class="mt-0.5 rounded border-slate-300 text-indigo-600 focus:ring-indigo-500"
				/>
				<div>
					<div class="flex items-center gap-1.5 text-xs font-bold text-slate-800 dark:text-slate-200">
						<UploadCloud class="w-3.5 h-3.5 text-indigo-500" />
						<span>Allow Peer Uploads</span>
					</div>
					<p class="text-[11px] text-slate-500 mt-0.5">
						Peers on localnet can upload files and drop them into the folder.
					</p>
				</div>
			</label>

			<!-- Allow Peer Deletion -->
			<label class="flex items-start gap-3 p-3 rounded-xl border border-slate-200 dark:border-slate-800 bg-slate-50/50 dark:bg-slate-950/40 cursor-pointer hover:border-indigo-300 transition-colors">
				<input
					type="checkbox"
					bind:checked={allowDelete}
					onchange={() => {
						if (isServerRunning) updateServerConfig();
					}}
					class="mt-0.5 rounded border-slate-300 text-rose-600 focus:ring-rose-500"
				/>
				<div>
					<div class="flex items-center gap-1.5 text-xs font-bold text-slate-800 dark:text-slate-200">
						<Trash2 class="w-3.5 h-3.5 text-rose-500" />
						<span>Allow Peer Deletions</span>
					</div>
					<p class="text-[11px] text-slate-500 mt-0.5">
						Peers can delete files and subfolders from the web portal.
					</p>
				</div>
			</label>
		</div>

		<!-- PIN Protection Option -->
		<div class="pt-3 border-t border-slate-100 dark:border-slate-800 space-y-3">
			<div class="flex items-center justify-between">
				<label class="flex items-center gap-2.5 cursor-pointer">
					<input
						type="checkbox"
						bind:checked={requirePin}
						onchange={() => {
							if (isServerRunning) updateServerConfig();
						}}
						class="rounded border-slate-300 text-indigo-600 focus:ring-indigo-500"
					/>
					<span class="text-xs font-bold text-slate-800 dark:text-slate-200">
						Require PIN Authentication
					</span>
				</label>

				{#if requirePin}
					<button
						onclick={regeneratePin}
						class="text-xs text-indigo-600 dark:text-indigo-400 hover:underline flex items-center gap-1 font-medium"
					>
						<RefreshCw class="w-3 h-3" />
						<span>Regenerate PIN</span>
					</button>
				{/if}
			</div>

			{#if requirePin}
				<div class="flex items-center gap-2 max-w-xs">
					<div class="relative flex-1">
						<input
							type={showPin ? 'text' : 'password'}
							bind:value={pin}
							onchange={() => {
								if (isServerRunning) updateServerConfig();
							}}
							placeholder="Enter PIN"
							class="w-full px-3 py-1.5 text-xs font-mono rounded-lg border border-slate-300 dark:border-slate-700 bg-slate-50 dark:bg-slate-950 text-slate-800 dark:text-slate-100 outline-none"
						/>
					</div>
					<button
						onclick={() => (showPin = !showPin)}
						class="p-2 rounded-lg border border-slate-200 dark:border-slate-700 text-slate-500 hover:bg-slate-100 dark:hover:bg-slate-800"
						title={showPin ? 'Hide PIN' : 'Show PIN'}
					>
						{#if showPin}
							<EyeOff class="w-3.5 h-3.5" />
						{:else}
							<Eye class="w-3.5 h-3.5" />
						{/if}
					</button>
				</div>
				<p class="text-[11px] text-slate-500 dark:text-slate-400">
					💡 <strong>Login Hint:</strong> When prompted in browser, leave <em>Username</em> blank (not required) and enter only the <em>Password</em>.
				</p>
			{/if}
		</div>
	</div>

	<!-- Folder Contents Preview -->
	<div class="bg-white dark:bg-slate-900/90 border border-slate-200 dark:border-slate-800 rounded-2xl p-4 sm:p-5 shadow-xs space-y-3">
		<div class="flex items-center justify-between">
			<h3 class="text-sm font-bold text-slate-900 dark:text-white flex items-center gap-2">
				<FileText class="w-4 h-4 text-slate-400" />
				<span>Contents Preview ({folderEntries.length} items)</span>
			</h3>

			<button
				onclick={() => loadPreview(selectedFolderPath)}
				disabled={isLoadingPreview}
				class="text-xs text-slate-500 hover:text-indigo-600 dark:hover:text-indigo-400 flex items-center gap-1"
			>
				<RefreshCw class="w-3 h-3 {isLoadingPreview ? 'animate-spin' : ''}" />
				<span>Refresh</span>
			</button>
		</div>

		{#if folderEntries.length === 0}
			<div class="p-6 text-center text-slate-400 text-xs">
				No files or subfolders found in this directory.
			</div>
		{:else}
			<div class="max-h-56 overflow-y-auto space-y-1 pr-1">
				{#each folderEntries as item}
					<div class="flex items-center justify-between p-2 rounded-lg hover:bg-slate-50 dark:hover:bg-slate-800/60 transition-colors text-xs">
						<div class="flex items-center gap-2 truncate">
							{#if item.isDir}
								<Folder class="w-3.5 h-3.5 text-amber-500 flex-shrink-0" />
							{:else}
								<FileText class="w-3.5 h-3.5 text-indigo-500 flex-shrink-0" />
							{/if}
							<span class="truncate font-medium text-slate-700 dark:text-slate-300">
								{item.name}
							</span>
						</div>
						<span class="text-[11px] font-mono text-slate-400 flex-shrink-0">
							{item.isDir ? 'Folder' : formatBytes(item.size)}
						</span>
					</div>
				{/each}
			</div>
		{/if}
	</div>
</div>

<!-- Directory Picker Modal (Especially enabling Android folder picking) -->
<DirectoryPickerModal
	isOpen={showFolderPickerModal}
	initialPath={selectedFolderPath}
	onClose={() => (showFolderPickerModal = false)}
	onSelect={onFolderSelected}
/>

<!-- QR Code Modal -->
{#if activeQrUrl}
	<div
		class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 dark:bg-black/75 backdrop-blur-sm p-4 animate-in fade-in duration-150"
	>
		<div
			class="w-full max-w-sm bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-800 rounded-2xl shadow-2xl p-6 space-y-4 animate-in zoom-in-95 duration-150 text-center"
		>
			<div class="flex items-center justify-between border-b border-slate-200 dark:border-slate-800 pb-3">
				<h3 class="text-sm font-semibold text-slate-900 dark:text-slate-100">Scan QR Code</h3>
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
