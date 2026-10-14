<script lang="ts">
	import { appState } from '$lib/state.svelte';
	import { formatBytes, formatSpeed, formatEta } from '$lib/format';
	import {
		ArrowUpRight,
		ArrowDownLeft,
		CheckCircle2,
		XCircle,
		AlertCircle,
		Activity,
		X,
		FileText
	} from 'lucide-svelte';

	function getStatusBadge(status: string) {
		switch (status) {
			case 'active':
				return { label: 'Transferring', color: 'bg-indigo-500/10 text-indigo-600 dark:text-indigo-400 border-indigo-500/20' };
			case 'completed':
				return { label: 'Completed', color: 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border-emerald-500/20' };
			case 'cancelled':
				return { label: 'Cancelled', color: 'bg-slate-200 dark:bg-slate-700/30 text-slate-600 dark:text-slate-400 border-slate-300 dark:border-slate-700/50' };
			case 'failed':
			default:
				return { label: 'Failed', color: 'bg-rose-500/10 text-rose-600 dark:text-rose-400 border-rose-500/20' };
		}
	}
</script>

<div class="flex flex-col h-full overflow-hidden p-4 sm:p-6 gap-4 sm:gap-6">
	<!-- Header -->
	<div class="flex items-center justify-between">
		<div class="flex items-center gap-3">
			<div class="p-2 sm:p-2.5 rounded-xl bg-violet-500/10 border border-violet-500/20 text-violet-600 dark:text-violet-400">
				<Activity class="w-5 h-5 sm:w-6 sm:h-6" />
			</div>
			<div>
				<h2 class="text-lg sm:text-xl font-bold text-slate-900 dark:text-slate-100 flex items-center gap-2">
					Transfer Queue
					<span
						class="text-xs px-2 py-0.5 rounded-full bg-slate-200 dark:bg-slate-800 text-slate-700 dark:text-slate-400 border border-slate-300 dark:border-slate-700/60 font-mono"
					>
						{appState.transfers.length}
					</span>
				</h2>
				<p class="text-[11px] sm:text-xs text-slate-500 dark:text-slate-400">Real-time session progress and speedometers</p>
			</div>
		</div>
	</div>

	<!-- Queue List -->
	<div class="flex-1 overflow-y-auto pr-1 space-y-3 sm:space-y-4">
		{#if appState.transfers.length === 0}
			<div
				class="h-full flex flex-col items-center justify-center border border-dashed border-slate-300 dark:border-slate-800 rounded-2xl p-6 sm:p-8 text-center text-slate-400 dark:text-slate-500 gap-3"
			>
				<FileText class="w-10 h-10 sm:w-12 sm:h-12 text-slate-400 dark:text-slate-600 stroke-[1.5]" />
				<div class="space-y-1">
					<p class="text-sm font-medium text-slate-600 dark:text-slate-400">No active or past transfers</p>
					<p class="text-xs text-slate-400 dark:text-slate-500 max-w-sm">
						Send files to a peer on the radar or wait for an incoming file request.
					</p>
				</div>
			</div>
		{:else}
			{#each appState.transfers as transfer (transfer.id)}
				{@const badge = getStatusBadge(transfer.status)}
				{@const percent =
					transfer.totalBytes > 0
						? Math.min(100, Math.round((transfer.bytesTransferred / transfer.totalBytes) * 100))
						: 0}

				<div
					class="bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-800 rounded-xl p-3.5 sm:p-5 shadow-sm space-y-3 sm:space-y-4 hover:border-slate-300 dark:hover:border-slate-700/80 transition-colors"
				>
					<!-- Header Row -->
					<div class="flex items-start justify-between">
						<div class="flex items-center gap-2.5 sm:gap-3">
							<div
								class="p-2 rounded-lg {transfer.direction === 'outgoing'
									? 'bg-blue-500/10 text-blue-600 dark:text-blue-400 border border-blue-500/20'
									: 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border border-emerald-500/20'}"
							>
								{#if transfer.direction === 'outgoing'}
									<ArrowUpRight class="w-4 h-4 sm:w-5 sm:h-5" />
								{:else}
									<ArrowDownLeft class="w-4 h-4 sm:w-5 sm:h-5" />
								{/if}
							</div>
							<div>
								<div class="flex items-center gap-2">
									<h3 class="font-semibold text-slate-900 dark:text-slate-200 text-sm truncate max-w-[130px] sm:max-w-none">{transfer.peerAlias}</h3>
									<span
										class="text-[10px] font-medium px-2 py-0.5 rounded-full border {badge.color}"
									>
										{badge.label}
									</span>
								</div>
								<p class="text-xs text-slate-500 dark:text-slate-400 truncate max-w-[150px] sm:max-w-none">
									{transfer.direction === 'outgoing' ? 'Sending to' : 'Receiving from'}
									{transfer.peerEndpoint || 'LAN Peer'}
								</p>
							</div>
						</div>

						<!-- Cancel Button if Active -->
						{#if transfer.status === 'active'}
							<button
								onclick={() => appState.cancelTransfer(transfer.id)}
								class="p-1.5 rounded-lg text-slate-400 hover:text-rose-600 dark:hover:text-rose-400 hover:bg-rose-500/10 transition-colors"
								title="Cancel Transfer"
							>
								<X class="w-4 h-4" />
							</button>
						{/if}
					</div>

					<!-- Progress Bar & Speedometer -->
					<div class="space-y-1.5 sm:space-y-2">
						<div class="flex flex-col sm:flex-row sm:items-center justify-between text-xs gap-1">
							<span class="text-slate-600 dark:text-slate-400 font-mono text-[11px] sm:text-xs">
								{formatBytes(transfer.bytesTransferred)} / {formatBytes(transfer.totalBytes)}
								<span class="text-slate-400 dark:text-slate-500 ml-1 font-semibold">({percent}%)</span>
							</span>

							{#if transfer.status === 'active'}
								<div class="flex items-center gap-2 sm:gap-3 font-mono text-slate-700 dark:text-slate-300 text-[11px] sm:text-xs">
									<span class="text-indigo-600 dark:text-indigo-400 font-medium">
										{formatSpeed(transfer.speedBytesPerSec)}
									</span>
									<span class="text-slate-300 dark:text-slate-600">|</span>
									<span class="text-slate-500 dark:text-slate-400">ETA: {formatEta(transfer.etaSeconds)}</span>
								</div>
							{/if}
						</div>

						<!-- Progress Track -->
						<div class="w-full bg-slate-200 dark:bg-slate-800 rounded-full h-2 overflow-hidden">
							<div
								class="h-full rounded-full transition-all duration-200 {transfer.status === 'failed'
									? 'bg-rose-500'
									: transfer.status === 'completed'
										? 'bg-emerald-500'
										: 'bg-indigo-600 dark:bg-indigo-500'}"
								style:width="{percent}%"
							></div>
						</div>
					</div>

					<!-- Files List Details -->
					{#if transfer.files.length > 0}
						<div class="pt-2 border-t border-slate-100 dark:border-slate-800/80 space-y-1.5">
							{#each transfer.files as f}
								<div class="flex items-center justify-between text-xs text-slate-500 dark:text-slate-400">
									<div class="flex items-center gap-2 truncate max-w-[65%] sm:max-w-[70%]">
										<FileText class="w-3.5 h-3.5 text-slate-400 dark:text-slate-500 flex-shrink-0" />
										<span class="truncate text-slate-800 dark:text-slate-300">{f.name}</span>
									</div>
									<div class="flex items-center gap-2 font-mono text-[11px]">
										<span>{formatBytes(f.size)}</span>
										{#if f.status === 'completed'}
											<CheckCircle2 class="w-3.5 h-3.5 text-emerald-500 dark:text-emerald-400" />
										{:else if f.status === 'failed'}
											<AlertCircle class="w-3.5 h-3.5 text-rose-500 dark:text-rose-400" />
										{/if}
									</div>
								</div>
							{/each}
						</div>
					{/if}
				</div>
			{/each}
		{/if}
	</div>
</div>
