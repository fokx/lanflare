<script lang="ts">
	import { appState } from '$lib/state.svelte';
	import { themeManager } from '$lib/theme.svelte';
	import { updateConfig, regenerateIdentity, generateRandomAlias, isTauri, isAndroid } from '$lib/api';
	import { open } from '@tauri-apps/plugin-dialog';
	import {
		Settings,
		Folder,
		FolderOpen,
		KeyRound,
		Radio,
		Check,
		X,
		Palette,
		Sun,
		Moon,
		Monitor,
		History,
		AlertCircle,
		Shield,
		Copy,
		Dices,
		RefreshCw
	} from 'lucide-svelte';

	let alias = $state(appState.status?.alias || '');
	let port = $state(appState.status?.port || 53317);
	let downloadDir = $state(appState.status?.downloadDir || '');
	let autoAccept = $state(appState.status?.autoAccept || false);
	let saveToHistory = $state(appState.status?.saveToHistory ?? true);
	let multicastAddress = $state(appState.status?.multicastAddress || '224.0.0.167');
	let pin = $state('');
	let pinEnabled = $state(appState.status?.pinEnabled || false);
	let isSaving = $state(false);
	let saveSuccess = $state(false);
	let errorMsg = $state<string | null>(null);
	let hasStoragePermission = $state(true);
	let copiedFp = $state(false);
	let isGeneratingAlias = $state(false);
	let isRegeneratingCert = $state(false);
	let showCertConfirm = $state(false);

	async function handleGenerateRandomAlias() {
		isGeneratingAlias = true;
		try {
			const newAlias = await generateRandomAlias();
			if (newAlias) {
				alias = newAlias;
			}
		} catch (e) {
			console.error('Failed to generate random alias:', e);
		} finally {
			isGeneratingAlias = false;
		}
	}

	async function handleRegenerateCertificate() {
		if (!showCertConfirm) {
			showCertConfirm = true;
			return;
		}
		showCertConfirm = false;
		isRegeneratingCert = true;
		try {
			const updated = await regenerateIdentity();
			appState.status = updated;
		} catch (e) {
			errorMsg = 'Failed to regenerate certificate: ' + String(e);
		} finally {
			isRegeneratingCert = false;
		}
	}

	async function handleCopyFingerprint() {
		if (!appState.status?.fingerprint) return;
		try {
			await navigator.clipboard.writeText(appState.status.fingerprint);
			copiedFp = true;
			setTimeout(() => {
				copiedFp = false;
			}, 2000);
		} catch (e) {
			console.error('Failed to copy fingerprint:', e);
		}
	}

	function checkStoragePermission() {
		if (typeof window !== 'undefined' && (window as any).AndroidStorage?.isStorageManager) {
			hasStoragePermission = (window as any).AndroidStorage.isStorageManager();
		}
	}

	function requestPermission() {
		if (typeof window !== 'undefined' && (window as any).AndroidStorage?.requestStoragePermission) {
			(window as any).AndroidStorage.requestStoragePermission();
			setTimeout(checkStoragePermission, 1500);
		}
	}

	$effect(() => {
		if (appState.showSettings && isAndroid()) {
			checkStoragePermission();
		}
	});

	$effect(() => {
		if (appState.status) {
			alias = appState.status.alias;
			port = appState.status.port;
			downloadDir = appState.status.downloadDir;
			autoAccept = appState.status.autoAccept;
			saveToHistory = appState.status.saveToHistory ?? true;
			multicastAddress = appState.status.multicastAddress || '224.0.0.167';
			pinEnabled = appState.status.pinEnabled;
		}
	});


	async function pickDownloadDirectory() {
		if (isAndroid()) {
			alert('Custom directory selection is not supported on Android due to Scoped Storage.');
			return;
		}
		if (isTauri()) {
			try {
				const selected = await open({
					directory: true,
					multiple: false,
					defaultPath: downloadDir || undefined
				});
				if (selected && typeof selected === 'string') {
					downloadDir = selected;
				}
			} catch (e) {
				console.error('Directory picker error:', e);
			}
		} else {
			// Mock in browser
			downloadDir = '/home/user/CustomDownloads';
		}
	}

	async function handleSave() {
		errorMsg = null;

		// Validate multicast IP address
		const mcTrimmed = multicastAddress.trim();
		const mcParts = mcTrimmed.split('.');
		if (mcParts.length !== 4 || mcParts.some((p) => isNaN(Number(p)) || Number(p) < 0 || Number(p) > 255)) {
			errorMsg = 'Invalid multicast address format (e.g., 224.0.0.167)';
			return;
		}
		const firstOctet = Number(mcParts[0]);
		if (firstOctet < 224 || firstOctet > 239) {
			errorMsg = 'Multicast address must be within 224.0.0.0 to 239.255.255.255';
			return;
		}

		isSaving = true;
		saveSuccess = false;
		try {
			const updated = await updateConfig({
				alias,
				port,
				downloadDir,
				autoAccept,
				saveToHistory,
				multicastAddress: mcTrimmed,
				pin: pinEnabled ? (pin ? pin : undefined) : null
			});
			appState.status = updated;
			saveSuccess = true;
			setTimeout(() => {
				saveSuccess = false;
				appState.showSettings = false;
			}, 600);
		} catch (e) {
			errorMsg = String(e);
		} finally {
			isSaving = false;
		}
	}
</script>

{#if appState.showSettings}
	<div
		class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 dark:bg-black/75 backdrop-blur-sm p-4 animate-in fade-in duration-150"
		style="padding-top: calc(1rem + var(--safe-top)); padding-bottom: calc(1rem + var(--safe-bottom)); padding-left: calc(1rem + var(--safe-left)); padding-right: calc(1rem + var(--safe-right));"
	>
		<div
			class="w-full max-w-lg bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-800 rounded-2xl shadow-2xl p-6 space-y-6 animate-in zoom-in-95 duration-150 max-h-[90vh] flex flex-col"
		>
			<!-- Header -->
			<div class="flex items-center justify-between border-b border-slate-200 dark:border-slate-800 pb-4 flex-none">
				<div class="flex items-center gap-3">
					<div class="p-2 rounded-xl bg-slate-100 dark:bg-slate-800 text-slate-700 dark:text-slate-300">
						<Settings class="w-5 h-5" />
					</div>
					<div>
						<h3 class="text-base font-semibold text-slate-900 dark:text-slate-100">Preferences & Node Identity</h3>
						<p class="text-xs text-slate-500 dark:text-slate-400">Configure theme, device alias, security, and folders</p>
					</div>
				</div>
				<button
					onclick={() => (appState.showSettings = false)}
					class="p-1 rounded-lg text-slate-400 hover:text-slate-700 dark:hover:text-slate-200 hover:bg-slate-100 dark:hover:bg-slate-800 transition-colors"
				>
					<X class="w-5 h-5" />
				</button>
			</div>

			<!-- Settings Form (Scrollable) -->
			<div class="space-y-4 text-sm overflow-y-auto pr-1 flex-1">
				<!-- Appearance / Theme Selector -->
				<div class="space-y-1.5">
					<span class="text-xs font-medium text-slate-700 dark:text-slate-300 block flex items-center gap-1.5">
						<Palette class="w-3.5 h-3.5 text-indigo-500" />
						<span>Theme Appearance</span>
					</span>
					<div class="grid grid-cols-3 gap-2">
						<button
							type="button"
							onclick={() => themeManager.setMode('system')}
							class="flex items-center justify-center gap-2 py-2 px-3 rounded-lg text-xs font-medium border transition-all cursor-pointer {themeManager.current ===
							'system'
								? 'bg-indigo-50 dark:bg-indigo-600/20 text-indigo-600 dark:text-indigo-400 border-indigo-300 dark:border-indigo-500/50 shadow-sm'
								: 'bg-slate-50 dark:bg-slate-950 text-slate-600 dark:text-slate-400 border-slate-200 dark:border-slate-800 hover:bg-slate-100 dark:hover:bg-slate-900'}"
						>
							<Monitor class="w-3.5 h-3.5" />
							<span>Auto</span>
						</button>
						<button
							type="button"
							onclick={() => themeManager.setMode('light')}
							class="flex items-center justify-center gap-2 py-2 px-3 rounded-lg text-xs font-medium border transition-all cursor-pointer {themeManager.current ===
							'light'
								? 'bg-indigo-50 dark:bg-indigo-600/20 text-indigo-600 dark:text-indigo-400 border-indigo-300 dark:border-indigo-500/50 shadow-sm'
								: 'bg-slate-50 dark:bg-slate-950 text-slate-600 dark:text-slate-400 border-slate-200 dark:border-slate-800 hover:bg-slate-100 dark:hover:bg-slate-900'}"
						>
							<Sun class="w-3.5 h-3.5" />
							<span>Light</span>
						</button>
						<button
							type="button"
							onclick={() => themeManager.setMode('dark')}
							class="flex items-center justify-center gap-2 py-2 px-3 rounded-lg text-xs font-medium border transition-all cursor-pointer {themeManager.current ===
							'dark'
								? 'bg-indigo-50 dark:bg-indigo-600/20 text-indigo-600 dark:text-indigo-400 border-indigo-300 dark:border-indigo-500/50 shadow-sm'
								: 'bg-slate-50 dark:bg-slate-950 text-slate-600 dark:text-slate-400 border-slate-200 dark:border-slate-800 hover:bg-slate-100 dark:hover:bg-slate-900'}"
						>
							<Moon class="w-3.5 h-3.5" />
							<span>Dark</span>
						</button>
					</div>
				</div>

				<!-- Device Alias -->
				<div class="space-y-1.5">
					<div class="flex items-center justify-between">
						<label for="alias-input" class="text-xs font-medium text-slate-700 dark:text-slate-300">Device Alias (Name on Network)</label>
						<button
							type="button"
							onclick={handleGenerateRandomAlias}
							disabled={isGeneratingAlias}
							class="flex items-center gap-1 text-[11px] font-medium text-indigo-600 dark:text-indigo-400 hover:text-indigo-500 transition-colors cursor-pointer disabled:opacity-50"
							title="Generate a friendly random two-word device name"
						>
							<Dices class="w-3.5 h-3.5 {isGeneratingAlias ? 'animate-spin' : ''}" />
							<span>Random Alias</span>
						</button>
					</div>
					<input
						id="alias-input"
						type="text"
						bind:value={alias}
						placeholder="e.g. Golden Mango"
						class="w-full bg-slate-50 dark:bg-slate-950 border border-slate-300 dark:border-slate-800 rounded-lg px-3 py-2 text-sm text-slate-900 dark:text-slate-200 focus:outline-none focus:border-indigo-500"
					/>
				</div>

				<!-- Device Fingerprint / Security Identity -->
				<div class="p-3 rounded-xl bg-slate-50 dark:bg-slate-950/60 border border-slate-200 dark:border-slate-800/80 space-y-2">
					<div class="flex items-center justify-between">
						<div class="flex items-center gap-1.5 text-xs font-medium text-slate-800 dark:text-slate-200">
							<Shield class="w-3.5 h-3.5 text-indigo-500" />
							<span>Device Certificate Fingerprint</span>
						</div>
						<div class="flex items-center gap-2">
							{#if appState.status?.fingerprint}
								<button
									type="button"
									onclick={handleCopyFingerprint}
									class="flex items-center gap-1 text-[11px] font-medium text-indigo-600 dark:text-indigo-400 hover:text-indigo-500 transition-colors cursor-pointer"
								>
									{#if copiedFp}
										<Check class="w-3.5 h-3.5 text-emerald-500" />
										<span class="text-emerald-500">Copied!</span>
									{:else}
										<Copy class="w-3.5 h-3.5" />
										<span>Copy Full</span>
									{/if}
								</button>
							{/if}
							<button
								type="button"
								onclick={handleRegenerateCertificate}
								disabled={isRegeneratingCert}
								class="flex items-center gap-1 text-[11px] font-medium text-amber-600 dark:text-amber-400 hover:text-amber-500 transition-colors cursor-pointer disabled:opacity-50"
								title="Regenerate self-signed TLS certificate and fingerprint"
							>
								<RefreshCw class="w-3.5 h-3.5 {isRegeneratingCert ? 'animate-spin' : ''}" />
								<span>Regenerate</span>
							</button>
						</div>
					</div>

					{#if showCertConfirm}
						<div class="p-2.5 rounded-lg bg-amber-50 dark:bg-amber-950/40 border border-amber-300 dark:border-amber-800/60 flex items-center justify-between gap-2 text-xs text-amber-800 dark:text-amber-300 animate-in fade-in duration-150">
							<span>Regenerating will update your TLS identity across the network. Proceed?</span>
							<div class="flex items-center gap-1.5 flex-shrink-0">
								<button
									type="button"
									onclick={handleRegenerateCertificate}
									class="px-2 py-1 rounded bg-amber-600 hover:bg-amber-700 text-white font-medium cursor-pointer"
								>
									Yes, Regenerate
								</button>
								<button
									type="button"
									onclick={() => (showCertConfirm = false)}
									class="px-2 py-1 rounded bg-slate-200 dark:bg-slate-800 text-slate-700 dark:text-slate-300 hover:bg-slate-300 dark:hover:bg-slate-700 cursor-pointer"
								>
									Cancel
								</button>
							</div>
						</div>
					{/if}

					<div class="p-2 rounded-lg bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-800 font-mono text-[10px] text-slate-600 dark:text-slate-400 break-all select-all">
						{appState.status?.fingerprint || 'Unavailable'}
					</div>
					<p class="text-[10px] text-slate-500 dark:text-slate-400">
						Unique SHA-256 fingerprint identifying this device for mutual TLS (mTLS) encryption and pinning. Persisted across restarts.
					</p>
				</div>

				<!-- Download Directory (Folder Picker) -->
				<div class="space-y-1.5">
					<label for="download-input" class="text-xs font-medium text-slate-700 dark:text-slate-300 flex items-center gap-1.5">
						<Folder class="w-3.5 h-3.5 text-indigo-500" />
						<span>Download Directory</span>
					</label>
					<div class="flex items-center gap-2">
						<div
							id="download-input"
							class="flex-1 bg-slate-50 dark:bg-slate-950 border border-slate-300 dark:border-slate-800 rounded-lg px-3 py-2 text-xs font-mono text-slate-800 dark:text-slate-200 truncate select-all"
							title={downloadDir}
						>
							<span class="truncate block">{downloadDir || 'Default download folder'}</span>
						</div>
						{#if !isAndroid()}
							<button
								type="button"
								onclick={pickDownloadDirectory}
								class="flex items-center gap-1.5 px-3 py-2 rounded-lg bg-indigo-50 dark:bg-indigo-600/20 hover:bg-indigo-100 dark:hover:bg-indigo-600/30 text-indigo-600 dark:text-indigo-400 border border-indigo-200 dark:border-indigo-500/30 text-xs font-medium cursor-pointer transition-colors flex-shrink-0"
							>
								<FolderOpen class="w-4 h-4" />
								<span>Choose Folder...</span>
							</button>
						{/if}
					</div>
					{#if isAndroid() && !hasStoragePermission}
						<div class="mt-1.5 p-2.5 rounded-lg bg-amber-50 dark:bg-amber-950/40 border border-amber-200 dark:border-amber-900/50 flex items-center justify-between gap-2">
							<span class="text-xs text-amber-800 dark:text-amber-300">
								Storage access needed to save files to Download folder.
							</span>
							<button
								type="button"
								onclick={requestPermission}
								class="px-2.5 py-1 text-xs font-semibold rounded bg-amber-600 hover:bg-amber-700 text-white shadow-xs cursor-pointer flex-shrink-0"
							>
								Grant Permission
							</button>
						</div>
					{/if}
					<p class="text-[11px] text-slate-400 dark:text-slate-500">
						{isAndroid() ? 'Incoming files are saved to your device Download folder.' : 'Incoming files and folders will be saved to this folder.'}
					</p>
				</div>


				<!-- Multicast Group Address & Port -->
				<div class="grid grid-cols-1 sm:grid-cols-2 gap-3 sm:gap-4">
					<div class="space-y-1.5">
						<label for="multicast-input" class="text-xs font-medium text-slate-700 dark:text-slate-300 flex items-center gap-1.5">
							<Radio class="w-3.5 h-3.5 text-indigo-500" />
							<span>Multicast Address</span>
						</label>
						<input
							id="multicast-input"
							type="text"
							bind:value={multicastAddress}
							placeholder="224.0.0.167"
							class="w-full bg-slate-50 dark:bg-slate-950 border border-slate-300 dark:border-slate-800 rounded-lg px-3 py-2 text-xs text-slate-900 dark:text-slate-200 focus:outline-none focus:border-indigo-500 font-mono"
						/>
					</div>

					<div class="space-y-1.5">
						<label for="port-input" class="text-xs font-medium text-slate-700 dark:text-slate-300">Server Port</label>
						<input
							id="port-input"
							type="number"
							bind:value={port}
							class="w-full bg-slate-50 dark:bg-slate-950 border border-slate-300 dark:border-slate-800 rounded-lg px-3 py-2 text-xs text-slate-900 dark:text-slate-200 focus:outline-none focus:border-indigo-500 font-mono"
						/>
					</div>
				</div>
				<p class="text-[11px] text-slate-400 dark:text-slate-500">
					Default multicast address is <code class="font-mono text-indigo-500">224.0.0.167</code>. Changing this allows isolation into custom peer discovery groups.
				</p>

				<!-- Save to History Toggle -->
				<div
					class="flex items-center justify-between p-3 rounded-xl bg-slate-50 dark:bg-slate-950/60 border border-slate-200 dark:border-slate-800/80"
				>
					<div class="space-y-0.5">
						<div class="text-xs font-medium text-slate-800 dark:text-slate-200 flex items-center gap-1.5">
							<History class="w-3.5 h-3.5 text-indigo-500" />
							<span>Save to History</span>
						</div>
						<p class="text-[11px] text-slate-500 dark:text-slate-400">
							Persist file transfer logs across application restarts (enabled by default)
						</p>
					</div>
					<input
						type="checkbox"
						bind:checked={saveToHistory}
						class="w-4 h-4 accent-indigo-600 rounded cursor-pointer"
					/>
				</div>

				<!-- Auto Accept Toggle -->
				<div
					class="flex items-center justify-between p-3 rounded-xl bg-slate-50 dark:bg-slate-950/60 border border-slate-200 dark:border-slate-800/80"
				>
					<div class="space-y-0.5">
						<div class="text-xs font-medium text-slate-800 dark:text-slate-200">Auto-Accept Transfers</div>
						<p class="text-[11px] text-slate-500 dark:text-slate-400">
							Automatically download incoming files without showing approval prompt
						</p>
					</div>
					<input
						type="checkbox"
						bind:checked={autoAccept}
						class="w-4 h-4 accent-indigo-600 rounded cursor-pointer"
					/>
				</div>

				<!-- Security PIN -->
				<div class="p-3 rounded-xl bg-slate-50 dark:bg-slate-950/60 border border-slate-200 dark:border-slate-800/80 space-y-3">
					<div class="flex items-center justify-between">
						<div class="space-y-0.5">
							<div class="text-xs font-medium text-slate-800 dark:text-slate-200 flex items-center gap-1.5">
								<KeyRound class="w-3.5 h-3.5 text-indigo-500" />
								<span>Require PIN Code</span>
							</div>
							<p class="text-[11px] text-slate-500 dark:text-slate-400">Protect this node with a required transfer PIN</p>
						</div>
						<input
							type="checkbox"
							bind:checked={pinEnabled}
							class="w-4 h-4 accent-indigo-600 rounded cursor-pointer"
						/>
					</div>

					{#if pinEnabled}
						<div class="pt-2 border-t border-slate-200 dark:border-slate-800/80">
							<input
								type="text"
								placeholder="Enter 4-8 digit numeric PIN"
								bind:value={pin}
								class="w-full bg-white dark:bg-slate-900 border border-slate-300 dark:border-slate-800 rounded-lg px-3 py-1.5 text-xs text-slate-900 dark:text-slate-200 focus:outline-none focus:border-indigo-500 font-mono"
							/>
						</div>
					{/if}
				</div>

				{#if errorMsg}
					<div class="flex items-center gap-2 p-2.5 rounded-lg bg-rose-500/10 border border-rose-500/20 text-rose-600 dark:text-rose-400 text-xs">
						<AlertCircle class="w-4 h-4 flex-shrink-0" />
						<span>{errorMsg}</span>
					</div>
				{/if}
			</div>

			<!-- Footer Buttons -->
			<div class="flex items-center justify-end gap-3 pt-2 border-t border-slate-200 dark:border-slate-800 flex-none">
				<button
					onclick={() => (appState.showSettings = false)}
					class="py-2 px-4 rounded-xl text-slate-600 dark:text-slate-400 hover:text-slate-900 dark:hover:text-slate-200 text-xs font-medium transition-colors"
				>
					Cancel
				</button>
				<button
					onclick={handleSave}
					disabled={isSaving}
					class="flex items-center gap-2 py-2 px-5 rounded-xl bg-indigo-600 hover:bg-indigo-500 text-white font-medium text-xs shadow-md shadow-indigo-600/20 transition-all cursor-pointer disabled:opacity-50"
				>
					{#if saveSuccess}
						<Check class="w-4 h-4" />
						<span>Saved!</span>
					{:else}
						<span>Save Preferences</span>
					{/if}
				</button>
			</div>
		</div>
	</div>
{/if}
