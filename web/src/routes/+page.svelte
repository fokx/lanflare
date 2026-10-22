<script lang="ts">
	import { onMount } from 'svelte';
	import { appState } from '$lib/state.svelte';
	import { themeManager } from '$lib/theme.svelte';
	import DeviceGrid from '$lib/components/DeviceGrid.svelte';
	import TransferQueue from '$lib/components/TransferQueue.svelte';
	import IncomingTransferModal from '$lib/components/IncomingTransferModal.svelte';
	import IncomingBrowserDownloadModal from '$lib/components/IncomingBrowserDownloadModal.svelte';
	import SettingsModal from '$lib/components/SettingsModal.svelte';
	import TextMessageModal from '$lib/components/TextMessageModal.svelte';
	import SendFileDialog from '$lib/components/SendFileDialog.svelte';
	import type { DiscoveredPeer } from '$lib/types';
	import {
		Flame,
		Radio,
		Activity,
		Settings,
		RefreshCw,
		Sun,
		Moon,
		Copy,
		Check,
		AlertTriangle
	} from 'lucide-svelte';
	import { announcePresence, getDiscoveredPeers, startReceiverServer } from '$lib/api';
	import { initShareTarget } from '$lib/shareTarget';
	import { shortenFingerprint } from '$lib/format';

	let isRefreshing = $state(false);
	let copiedFp = $state(false);
	let isRetryingServer = $state(false);

	async function handleManualRetryServer() {
		isRetryingServer = true;
		try {
			appState.serverPort = await startReceiverServer();
			appState.portConflict = false;
		} catch (e) {
			console.warn('Port still occupied:', e);
		} finally {
			setTimeout(() => {
				isRetryingServer = false;
			}, 600);
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

	onMount(() => {
		appState.init();
		const cleanupShareTarget = initShareTarget();

		async function handleWindowKeydown(e: KeyboardEvent) {
			if ((e.ctrlKey || e.metaKey) && (e.key === 'v' || e.key === 'V')) {
				const target = e.target as HTMLElement | null;
				if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable)) {
					return;
				}
				e.preventDefault();
				await appState.pasteFromClipboard();
			}
		}

		async function handleWindowPaste(e: ClipboardEvent) {
			const target = e.target as HTMLElement | null;
			if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable)) {
				return;
			}
			e.preventDefault();
			await appState.pasteFromClipboard();
		}

		window.addEventListener('keydown', handleWindowKeydown);
		window.addEventListener('paste', handleWindowPaste);

		return () => {
			cleanupShareTarget();
			window.removeEventListener('keydown', handleWindowKeydown);
			window.removeEventListener('paste', handleWindowPaste);
		};
	});


	async function handleRefresh() {
		isRefreshing = true;
		try {
			await announcePresence();
			appState.peers = await getDiscoveredPeers();
		} catch (e) {
			console.error('Failed to trigger announcement:', e);
		} finally {
			setTimeout(() => {
				isRefreshing = false;
			}, 800);
		}
	}

	function handleSelectPeer(peer: DiscoveredPeer) {
		appState.selectedPeer = peer;
		appState.showSendDialog = true;
	}

	function toggleTheme() {
		if (themeManager.isDark) {
			themeManager.setMode('light');
		} else {
			themeManager.setMode('dark');
		}
	}
</script>

<div class="flex flex-col md:flex-row h-screen h-[100dvh] w-screen overflow-hidden bg-slate-50 dark:bg-slate-950 font-sans text-slate-800 dark:text-slate-100 transition-colors duration-150">
	<!-- Mobile Top Navigation Bar (Visible only on < md screens) -->
	<header
		class="md:hidden flex-none flex items-center justify-between px-4 py-2.5 bg-white dark:bg-slate-950/90 border-b border-slate-200 dark:border-slate-900 z-20 shadow-xs"
		style="padding-top: calc(0.625rem + var(--safe-top)); padding-left: calc(1rem + var(--safe-left)); padding-right: calc(1rem + var(--safe-right));"
	>
		<div class="flex items-center gap-2.5">
			<div class="p-1.5 rounded-lg bg-gradient-to-br from-indigo-500 to-violet-600 text-white shadow-sm">
				<Flame class="w-5 h-5 fill-white/20 stroke-[2.2]" />
			</div>
			<div>
				<h1 class="font-bold text-sm tracking-tight text-slate-900 dark:text-white flex items-center gap-1.5 leading-none">
					LanFlare
					<span class="text-[9px] font-mono px-1 py-0.2 rounded bg-indigo-50 dark:bg-indigo-500/20 text-indigo-600 dark:text-indigo-400 border border-indigo-200 dark:border-indigo-500/30">
						v2.2
					</span>
				</h1>
			</div>
		</div>

		<!-- Mobile Header Quick Actions -->
		<div class="flex items-center gap-1.5">
			<button
				onclick={handleRefresh}
				disabled={isRefreshing}
				class="p-2 rounded-lg text-slate-600 dark:text-slate-400 hover:text-slate-900 dark:hover:text-white hover:bg-slate-100 dark:hover:bg-slate-800 transition-colors"
				title="Scan LAN"
			>
				<RefreshCw class="w-4 h-4 {isRefreshing ? 'animate-spin text-indigo-600 dark:text-indigo-400' : ''}" />
			</button>

			<button
				onclick={toggleTheme}
				class="p-2 rounded-lg text-slate-600 dark:text-slate-400 hover:text-slate-900 dark:hover:text-white hover:bg-slate-100 dark:hover:bg-slate-800 transition-colors"
				title={themeManager.isDark ? 'Switch to Light' : 'Switch to Dark'}
			>
				{#if themeManager.isDark}
					<Sun class="w-4 h-4 text-amber-400" />
				{:else}
					<Moon class="w-4 h-4 text-slate-600" />
				{/if}
			</button>

			<button
				onclick={() => (appState.showSettings = true)}
				class="p-2 rounded-lg text-slate-600 dark:text-slate-400 hover:text-slate-900 dark:hover:text-white hover:bg-slate-100 dark:hover:bg-slate-800 transition-colors"
				title="Preferences & Settings"
			>
				<Settings class="w-4 h-4" />
			</button>
		</div>
	</header>

	<!-- Desktop Left Sidebar (Hidden on mobile < md) -->
	<aside
		class="hidden md:flex w-64 border-r border-slate-200 dark:border-slate-900 bg-white dark:bg-slate-950/80 flex-col justify-between p-4 shadow-sm dark:shadow-none transition-colors duration-150 flex-none"
		style="padding-top: calc(1rem + var(--safe-top)); padding-bottom: calc(1rem + var(--safe-bottom)); padding-left: calc(1rem + var(--safe-left));"
	>
		<!-- Brand & Logo -->
		<div class="space-y-6">
			<div class="flex items-center gap-3 px-2 py-1">
				<div
					class="p-2 rounded-xl bg-gradient-to-br from-indigo-500 to-violet-600 text-white shadow-lg shadow-indigo-500/20"
				>
					<Flame class="w-6 h-6 fill-white/20 stroke-[2.2]" />
				</div>
				<div>
					<h1 class="font-bold text-base tracking-tight text-slate-900 dark:text-white flex items-center gap-1.5">
						LanFlare
						<span
							class="text-[10px] font-mono px-1.5 py-0.2 rounded bg-indigo-50 dark:bg-indigo-500/20 text-indigo-600 dark:text-indigo-400 border border-indigo-200 dark:border-indigo-500/30"
						>
							v2.2
						</span>
					</h1>
					<p class="text-[11px] text-slate-500 dark:text-slate-400">High-Performance LAN Share</p>
				</div>
			</div>

			<!-- Navigation Links -->
			<nav class="space-y-1">
				<button
					onclick={() => (appState.activeTab = 'radar')}
					class="w-full flex items-center justify-between px-3 py-2.5 rounded-xl text-xs font-medium transition-all {appState.activeTab ===
					'radar'
						? 'bg-indigo-50 dark:bg-indigo-600/15 text-indigo-600 dark:text-indigo-400 border border-indigo-200 dark:border-indigo-500/30 shadow-sm'
						: 'text-slate-600 dark:text-slate-400 hover:text-slate-900 dark:hover:text-slate-200 hover:bg-slate-100 dark:hover:bg-slate-900'}"
				>
					<div class="flex items-center gap-2.5">
						<Radio class="w-4 h-4 {appState.activeTab === 'radar' ? 'text-indigo-600 dark:text-indigo-400' : 'text-slate-400 dark:text-slate-500'}" />
						<span>Discovered Devices</span>
					</div>
					<span
						class="text-[10px] px-1.5 py-0.5 rounded-full font-mono {appState.activeTab === 'radar'
							? 'bg-indigo-100 dark:bg-indigo-500/20 text-indigo-700 dark:text-indigo-300'
							: 'bg-slate-200 dark:bg-slate-800 text-slate-600 dark:text-slate-400'}"
					>
						{appState.peers.length}
					</span>
				</button>

				<button
					onclick={() => (appState.activeTab = 'transfers')}
					class="w-full flex items-center justify-between px-3 py-2.5 rounded-xl text-xs font-medium transition-all {appState.activeTab ===
					'transfers'
						? 'bg-indigo-50 dark:bg-indigo-600/15 text-indigo-600 dark:text-indigo-400 border border-indigo-200 dark:border-indigo-500/30 shadow-sm'
						: 'text-slate-600 dark:text-slate-400 hover:text-slate-900 dark:hover:text-slate-200 hover:bg-slate-100 dark:hover:bg-slate-900'}"
				>
					<div class="flex items-center gap-2.5">
						<Activity
							class="w-4 h-4 {appState.activeTab === 'transfers' ? 'text-indigo-600 dark:text-indigo-400' : 'text-slate-400 dark:text-slate-500'}"
						/>
						<span>Transfer Queue</span>
					</div>
					{#if appState.activeTransferCount > 0}
						<span
							class="text-[10px] px-1.5 py-0.5 rounded-full font-mono bg-indigo-500 text-white animate-pulse"
						>
							{appState.activeTransferCount}
						</span>
					{/if}
				</button>
			</nav>
		</div>

		<!-- Bottom Status & Actions -->
		<div class="space-y-3 pt-4 border-t border-slate-200 dark:border-slate-900">
			<!-- Local Node Badge -->
			<div class="bg-slate-100 dark:bg-slate-900/80 border border-slate-200 dark:border-slate-800/80 rounded-xl p-3 space-y-1.5">
				<div class="flex items-center justify-between">
					<span class="text-[11px] font-semibold text-slate-700 dark:text-slate-300 truncate">
						{appState.status?.alias || 'This Device'}
					</span>
					{#if appState.portConflict}
						<div class="flex items-center gap-1 text-[10px] text-amber-600 dark:text-amber-400 font-medium" title="Port {appState.status?.port || 53317} is occupied by another instance. Auto-retrying...">
							<span class="w-1.5 h-1.5 rounded-full bg-amber-500 animate-ping"></span>
							<span>Port In Use</span>
						</div>
					{:else}
						<div class="flex items-center gap-1 text-[10px] text-emerald-600 dark:text-emerald-400 font-medium">
							<span class="w-1.5 h-1.5 rounded-full bg-emerald-500 animate-pulse"></span>
							<span>Port {appState.serverPort || appState.status?.port || 53317}</span>
						</div>
					{/if}
				</div>
				<p class="text-[10px] text-slate-500 font-mono truncate">
					{appState.status?.downloadDir || 'Downloads'}
				</p>
				{#if appState.status?.fingerprint}
					<div class="flex items-center justify-between text-[10px] text-slate-500 dark:text-slate-400 pt-1 border-t border-slate-200/60 dark:border-slate-800/60">
						<span>Fingerprint:</span>
						<button
							type="button"
							onclick={handleCopyFingerprint}
							class="flex items-center gap-1 font-mono text-[10px] text-slate-700 dark:text-slate-300 hover:text-indigo-600 dark:hover:text-indigo-400 transition-colors cursor-pointer group"
							title="Click to copy full SHA-256 fingerprint: {appState.status.fingerprint}"
						>
							<span>{shortenFingerprint(appState.status.fingerprint)}</span>
							{#if copiedFp}
								<Check class="w-3 h-3 text-emerald-500" />
							{:else}
								<Copy class="w-3 h-3 text-slate-400 group-hover:text-indigo-500" />
							{/if}
						</button>
					</div>
				{/if}
			</div>

			<!-- Quick Actions: Scan, Theme Toggle & Settings Buttons -->
			<div class="flex items-center gap-2">
				<button
					onclick={handleRefresh}
					disabled={isRefreshing}
					class="flex-1 flex items-center justify-center gap-1.5 py-2 px-3 rounded-lg bg-slate-100 dark:bg-slate-900 hover:bg-slate-200 dark:hover:bg-slate-850 border border-slate-200 dark:border-slate-800 text-slate-700 dark:text-slate-300 hover:text-slate-900 dark:hover:text-white text-xs font-medium transition-colors cursor-pointer"
					title="Announce Presence Burst"
				>
					<RefreshCw class="w-3.5 h-3.5 {isRefreshing ? 'animate-spin text-indigo-600 dark:text-indigo-400' : ''}" />
					<span>Scan</span>
				</button>

				<button
					onclick={toggleTheme}
					class="p-2 rounded-lg bg-slate-100 dark:bg-slate-900 hover:bg-slate-200 dark:hover:bg-slate-850 border border-slate-200 dark:border-slate-800 text-slate-700 dark:text-slate-300 hover:text-slate-900 dark:hover:text-white text-xs transition-colors cursor-pointer"
					title={themeManager.isDark ? 'Switch to Light Theme' : 'Switch to Dark Theme'}
				>
					{#if themeManager.isDark}
						<Sun class="w-4 h-4 text-amber-400" />
					{:else}
						<Moon class="w-4 h-4 text-slate-600" />
					{/if}
				</button>

				<button
					onclick={() => (appState.showSettings = true)}
					class="p-2 rounded-lg bg-slate-100 dark:bg-slate-900 hover:bg-slate-200 dark:hover:bg-slate-850 border border-slate-200 dark:border-slate-800 text-slate-700 dark:text-slate-300 hover:text-slate-900 dark:hover:text-white text-xs transition-colors cursor-pointer"
					title="Preferences & Settings"
				>
					<Settings class="w-4 h-4" />
				</button>
			</div>
		</div>
	</aside>

	<!-- Main Content Canvas -->
	<main
		class="flex-1 min-w-0 h-full overflow-hidden flex flex-col bg-slate-50 dark:bg-slate-950 transition-colors duration-150"
		style="padding-left: var(--safe-left); padding-right: var(--safe-right);"
	>
		{#if appState.portConflict}
			<div class="m-3 sm:m-4 mb-0 p-3 sm:p-3.5 rounded-xl bg-amber-50 dark:bg-amber-950/40 border border-amber-300 dark:border-amber-800/60 flex items-start justify-between gap-3 text-amber-900 dark:text-amber-200 shadow-xs transition-all flex-none">
				<div class="flex items-start gap-2.5">
					<AlertTriangle class="w-4 h-4 sm:w-5 sm:h-5 text-amber-600 dark:text-amber-400 flex-shrink-0 mt-0.5 animate-pulse" />
					<div class="space-y-0.5">
						<p class="text-xs font-semibold">
							Port {appState.status?.port || 53317} is occupied by another LocalSend or LanFlare instance
						</p>
						<p class="text-[11px] text-amber-700 dark:text-amber-300">
							LanFlare is actively monitoring this port and will automatically start receiving as soon as the other instance exits.
						</p>
					</div>
				</div>
				<button
					type="button"
					onclick={handleManualRetryServer}
					disabled={isRetryingServer}
					class="px-2.5 py-1 text-xs font-medium rounded-lg bg-amber-200 dark:bg-amber-900/60 hover:bg-amber-300 dark:hover:bg-amber-800 text-amber-900 dark:text-amber-100 transition-colors cursor-pointer flex-shrink-0 flex items-center gap-1"
				>
					<RefreshCw class="w-3 h-3 {isRetryingServer ? 'animate-spin' : ''}" />
					<span>Retry</span>
				</button>
			</div>
		{/if}

		{#if appState.activeTab === 'radar'}
			<DeviceGrid onSelectPeer={handleSelectPeer} />
		{:else}
			<TransferQueue />
		{/if}
	</main>

	<!-- Mobile Bottom Navigation Bar (Visible only on < md screens) -->
	<nav
		class="md:hidden flex-none flex items-center justify-around bg-white dark:bg-slate-950/95 border-t border-slate-200 dark:border-slate-900 px-3 py-2 z-20 shadow-md"
		style="padding-bottom: calc(0.5rem + var(--safe-bottom)); padding-left: calc(0.75rem + var(--safe-left)); padding-right: calc(0.75rem + var(--safe-right));"
	>
		<button
			onclick={() => (appState.activeTab = 'radar')}
			class="flex flex-col items-center gap-1 px-4 py-1.5 rounded-xl transition-all {appState.activeTab === 'radar'
				? 'text-indigo-600 dark:text-indigo-400 font-semibold'
				: 'text-slate-500 dark:text-slate-400'}"
		>
			<div class="relative">
				<Radio class="w-5 h-5" />
				{#if appState.peers.length > 0}
					<span class="absolute -top-1 -right-2 px-1 py-0.2 rounded-full text-[9px] font-mono bg-indigo-100 dark:bg-indigo-900 text-indigo-700 dark:text-indigo-300">
						{appState.peers.length}
					</span>
				{/if}
			</div>
			<span class="text-[10px]">Devices</span>
		</button>

		<button
			onclick={() => (appState.activeTab = 'transfers')}
			class="flex flex-col items-center gap-1 px-4 py-1.5 rounded-xl transition-all {appState.activeTab === 'transfers'
				? 'text-indigo-600 dark:text-indigo-400 font-semibold'
				: 'text-slate-500 dark:text-slate-400'}"
		>
			<div class="relative">
				<Activity class="w-5 h-5" />
				{#if appState.activeTransferCount > 0}
					<span class="absolute -top-1 -right-2 w-2 h-2 rounded-full bg-indigo-500 animate-pulse"></span>
				{/if}
			</div>
			<span class="text-[10px]">Transfers</span>
		</button>
	</nav>
</div>

<!-- Modals -->
<IncomingTransferModal />
<IncomingBrowserDownloadModal />
<SettingsModal />
<TextMessageModal />

{#if appState.showSendDialog && appState.selectedPeer}
	<SendFileDialog
		peer={appState.selectedPeer}
		onClose={() => {
			appState.showSendDialog = false;
			appState.activeTab = 'transfers';
		}}
	/>
{/if}
