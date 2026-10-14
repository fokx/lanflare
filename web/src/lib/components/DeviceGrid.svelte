<script lang="ts">
	import { appState } from '$lib/state.svelte';
	import { formatBytes, shortenFingerprint } from '$lib/format';
	import type { DiscoveredPeer } from '$lib/types';
	import {
		Laptop,
		Smartphone,
		Server,
		Globe,
		Radio,
		ShieldCheck,
		Search,
		Send,
		HardDrive
	} from 'lucide-svelte';

	let { onSelectPeer } = $props<{
		onSelectPeer: (peer: DiscoveredPeer) => void;
	}>();

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
</script>

<div class="flex flex-col h-full overflow-hidden p-6 gap-6">
	<!-- Header Bar -->
	<div class="flex items-center justify-between">
		<div class="flex items-center gap-3">
			<div class="p-2.5 rounded-xl bg-indigo-500/10 border border-indigo-500/20 text-indigo-600 dark:text-indigo-400">
				<Radio class="w-6 h-6 animate-pulse" />
			</div>
			<div>
				<h2 class="text-xl font-bold text-slate-900 dark:text-slate-100 flex items-center gap-2">
					Discovered Devices
					<span
						class="text-xs px-2 py-0.5 rounded-full bg-slate-200 dark:bg-slate-800 text-slate-700 dark:text-slate-400 border border-slate-300 dark:border-slate-700/60 font-mono"
					>
						{appState.peers.length}
					</span>
				</h2>
				<p class="text-xs text-slate-500 dark:text-slate-400">
					Active local peers on Multicast UDP & mDNS DNS-SD
				</p>
			</div>
		</div>

		<!-- Search Input -->
		<div class="relative w-64">
			<Search class="absolute left-3 top-2.5 w-4 h-4 text-slate-400 pointer-events-none" />
			<input
				type="text"
				placeholder="Filter devices or IP..."
				bind:value={appState.searchQuery}
				class="w-full bg-white dark:bg-slate-900 border border-slate-300 dark:border-slate-800 rounded-lg pl-9 pr-3 py-1.5 text-sm text-slate-900 dark:text-slate-200 placeholder-slate-400 dark:placeholder-slate-500 focus:outline-none focus:border-indigo-500 transition-colors shadow-sm dark:shadow-none"
			/>
		</div>
	</div>

	<!-- Peers Grid -->
	<div class="flex-1 overflow-y-auto pr-1">
		{#if appState.filteredPeers.length === 0}
			<div
				class="h-full flex flex-col items-center justify-center border border-dashed border-slate-300 dark:border-slate-800 rounded-2xl p-8 text-center text-slate-400 dark:text-slate-500 gap-3"
			>
				<Radio class="w-12 h-12 text-slate-400 dark:text-slate-600 stroke-[1.5]" />
				<div class="space-y-1">
					<p class="text-sm font-medium text-slate-600 dark:text-slate-400">Scanning local network...</p>
					<p class="text-xs text-slate-400 dark:text-slate-500 max-w-sm">
						Make sure other LocalSend or LanFlare devices are connected to the same Wi-Fi or subnet.
					</p>
				</div>
			</div>
		{:else}
			<div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
				{#each appState.filteredPeers as peer (peer.fingerprint)}
					{@const Icon = getDeviceIcon(peer.deviceType)}
					<div
						class="group relative bg-white dark:bg-slate-900/90 hover:bg-slate-50 dark:hover:bg-slate-850 border border-slate-200 dark:border-slate-800 hover:border-indigo-500/40 rounded-xl p-4.5 transition-all duration-200 flex flex-col justify-between shadow-sm hover:shadow-md dark:shadow-none dark:hover:shadow-indigo-500/5 hover:-translate-y-0.5"
					>
						<!-- Card Top -->
						<div class="flex items-start justify-between gap-3">
							<div class="flex items-center gap-3">
								<div
									class="p-2.5 rounded-lg bg-slate-100 dark:bg-slate-800 border border-slate-200 dark:border-slate-700/60 text-slate-700 dark:text-slate-300 group-hover:text-indigo-600 dark:group-hover:text-indigo-400 group-hover:bg-indigo-50 dark:group-hover:bg-indigo-500/10 group-hover:border-indigo-200 dark:group-hover:border-indigo-500/20 transition-colors"
								>
									<Icon class="w-5 h-5" />
								</div>
								<div>
									<h3 class="font-semibold text-slate-900 dark:text-slate-200 text-sm leading-snug group-hover:text-indigo-600 dark:group-hover:text-white transition-colors">
										{peer.alias}
									</h3>
									<p class="text-xs text-slate-500 dark:text-slate-400">
										{peer.deviceModel || peer.deviceType || 'Device'}
									</p>
								</div>
							</div>

							<!-- Security Badge -->
							<div
								class="flex items-center gap-1 text-[11px] px-2 py-0.5 rounded-md bg-emerald-500/10 border border-emerald-500/20 text-emerald-600 dark:text-emerald-400 font-medium"
								title="Mutual TLS verified with pinned SHA-256 fingerprint"
							>
								<ShieldCheck class="w-3.5 h-3.5" />
								<span>mTLS</span>
							</div>
						</div>

						<!-- Card Endpoints & Info -->
						<div class="my-4 pt-3 border-t border-slate-100 dark:border-slate-800/80 space-y-1.5 text-xs">
							<div class="flex items-center justify-between text-slate-500 dark:text-slate-400">
								<span>Endpoint:</span>
								<span class="font-mono text-slate-800 dark:text-slate-300">
									{peer.endpoints[0] || 'Unknown IP'}
								</span>
							</div>
							<div class="flex items-center justify-between text-slate-500 dark:text-slate-400">
								<span>Fingerprint:</span>
								<span
									class="font-mono text-slate-500 dark:text-slate-400 hover:text-slate-800 dark:hover:text-slate-200 transition-colors cursor-help"
									title={peer.fingerprint}
								>
									{shortenFingerprint(peer.fingerprint)}
								</span>
							</div>
						</div>

						<!-- Card Bottom Action -->
						<button
							onclick={() => onSelectPeer(peer)}
							class="w-full mt-1 flex items-center justify-center gap-2 py-2 px-3 rounded-lg bg-indigo-600 hover:bg-indigo-500 text-white font-medium text-xs shadow-sm hover:shadow-indigo-500/20 transition-all cursor-pointer"
						>
							<Send class="w-3.5 h-3.5" />
							<span>Send Files</span>
						</button>
					</div>
				{/each}
			</div>
		{/if}
	</div>
</div>
