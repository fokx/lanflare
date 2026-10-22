<script lang="ts">
	import { listDirectory, getCommonDirectories } from '$lib/api';
	import type { DirListingDto, QuickDirDto, DirEntryDto } from '$lib/types';
	import {
		Folder,
		FolderPlus,
		ChevronRight,
		ArrowLeft,
		X,
		Check,
		Search,
		FileText,
		HardDrive,
		RefreshCw,
		AlertCircle
	} from 'lucide-svelte';

	interface Props {
		isOpen: boolean;
		initialPath?: string;
		onClose: () => void;
		onSelect: (path: string) => void;
	}

	let { isOpen, initialPath, onClose, onSelect }: Props = $props();

	let currentPath = $state<string>('');
	let parentPath = $state<string | null>(null);
	let entries = $state<DirEntryDto[]>([]);
	let commonPaths = $state<QuickDirDto[]>([]);
	let isAndroid = $state<boolean>(false);
	let isLoading = $state<boolean>(false);
	let errorMessage = $state<string | null>(null);
	let searchFilter = $state<string>('');
	let isEditingPath = $state<boolean>(false);
	let manualPathInput = $state<string>('');

	// Load directory when modal is opened or path changes
	$effect(() => {
		if (isOpen) {
			loadPath(initialPath || currentPath || undefined);
		}
	});

	async function loadPath(targetPath?: string) {
		isLoading = true;
		errorMessage = null;
		searchFilter = '';
		try {
			const res: DirListingDto = await listDirectory(targetPath);
			currentPath = res.currentPath;
			parentPath = res.parentPath;
			entries = res.entries;
			isAndroid = res.isAndroid;
			commonPaths = res.commonPaths;
			manualPathInput = res.currentPath;
		} catch (err: any) {
			console.error('Failed to list directory:', err);
			errorMessage = String(err?.message || err);
		} finally {
			isLoading = false;
		}
	}

	function handleNavigate(path: string) {
		loadPath(path);
	}

	function handleGoUp() {
		if (parentPath) {
			loadPath(parentPath);
		}
	}

	function handleConfirm() {
		if (currentPath) {
			onSelect(currentPath);
			onClose();
		}
	}

	function handleManualPathSubmit(e: SubmitEvent) {
		e.preventDefault();
		if (manualPathInput.trim()) {
			isEditingPath = false;
			loadPath(manualPathInput.trim());
		}
	}

	// Filter directories
	let filteredEntries = $derived.by(() => {
		const q = searchFilter.trim().toLowerCase();
		if (!q) return entries;
		return entries.filter((e) => e.name.toLowerCase().includes(q));
	});

	// Only directories for selection
	let dirEntries = $derived(filteredEntries.filter((e) => e.isDir));

	// Breadcrumb path components
	let pathBreadcrumbs = $derived.by(() => {
		if (!currentPath) return [];
		const parts = currentPath.split('/').filter(Boolean);
		let acc = '';
		return parts.map((part) => {
			acc += '/' + part;
			return {
				name: part,
				fullPath: acc
			};
		});
	});

	let folderBasename = $derived.by(() => {
		if (!currentPath) return 'Selected Folder';
		const parts = currentPath.split('/').filter(Boolean);
		return parts.length > 0 ? parts[parts.length - 1] : currentPath;
	});
</script>

{#if isOpen}
	<div class="fixed inset-0 z-50 flex items-center justify-center p-3 sm:p-4 bg-slate-950/70 backdrop-blur-xs animate-in fade-in duration-150">
		<div
			class="relative w-full max-w-xl max-h-[90vh] bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-800 rounded-2xl shadow-2xl flex flex-col overflow-hidden text-slate-800 dark:text-slate-100"
		>
			<!-- Header -->
			<div class="flex items-center justify-between px-4 py-3.5 border-b border-slate-200 dark:border-slate-800 flex-none bg-slate-50 dark:bg-slate-950/50">
				<div class="flex items-center gap-2">
					<div class="p-1.5 rounded-lg bg-amber-500/10 text-amber-600 dark:text-amber-400">
						<Folder class="w-4 h-4 stroke-[2.2]" />
					</div>
					<div>
						<h3 class="font-bold text-sm tracking-tight text-slate-900 dark:text-white leading-none">
							Select Folder
						</h3>
						<p class="text-[11px] text-slate-500 mt-0.5">
							{isAndroid ? 'Browse internal storage or SD card' : 'Choose directory to serve or transfer'}
						</p>
					</div>
				</div>

				<button
					onclick={onClose}
					class="p-1.5 rounded-lg text-slate-400 hover:text-slate-600 dark:hover:text-slate-200 hover:bg-slate-100 dark:hover:bg-slate-800 transition-colors"
					title="Close"
				>
					<X class="w-4 h-4" />
				</button>
			</div>

			<!-- Quick Shortcuts (Especially crucial on Android) -->
			{#if commonPaths.length > 0}
				<div class="flex items-center gap-1.5 px-4 py-2 border-b border-slate-200 dark:border-slate-800/60 overflow-x-auto scrollbar-none flex-none bg-slate-50/50 dark:bg-slate-950/30">
					<span class="text-[10px] font-semibold text-slate-400 uppercase tracking-wider flex-shrink-0 mr-1">
						Quick:
					</span>
					{#each commonPaths as cp}
						<button
							onclick={() => loadPath(cp.path)}
							class="px-2.5 py-1 rounded-lg text-xs font-medium transition-all flex-shrink-0 cursor-pointer {currentPath === cp.path
								? 'bg-indigo-600 text-white shadow-xs'
								: 'bg-white dark:bg-slate-800 border border-slate-200 dark:border-slate-700/80 text-slate-600 dark:text-slate-300 hover:border-indigo-400'}"
						>
							{cp.label}
						</button>
					{/each}
				</div>
			{/if}

			<!-- Current Path / Breadcrumbs Bar -->
			<div class="px-4 py-2.5 border-b border-slate-200 dark:border-slate-800 flex items-center gap-2 flex-none bg-slate-100/60 dark:bg-slate-900/60">
				{#if parentPath}
					<button
						onclick={handleGoUp}
						class="p-1.5 rounded-lg border border-slate-200 dark:border-slate-700 bg-white dark:bg-slate-800 hover:bg-slate-50 dark:hover:bg-slate-750 text-slate-600 dark:text-slate-300 text-xs font-semibold flex-shrink-0 flex items-center gap-1"
						title="Go Up"
					>
						<ArrowLeft class="w-3.5 h-3.5" />
						<span class="hidden sm:inline">Up</span>
					</button>
				{/if}

				{#if !isEditingPath}
					<div class="flex-1 flex items-center gap-1 text-xs overflow-x-auto scrollbar-none py-1">
						<button
							onclick={() => loadPath('/')}
							class="text-slate-500 hover:text-indigo-600 dark:hover:text-indigo-400 font-mono flex-shrink-0"
						>
							/
						</button>
						{#each pathBreadcrumbs as crumb, i}
							<button
								onclick={() => loadPath(crumb.fullPath)}
								class="px-1.5 py-0.5 rounded hover:bg-slate-200 dark:hover:bg-slate-800 text-slate-700 dark:text-slate-200 font-medium flex-shrink-0 truncate max-w-[120px] transition-colors"
								title={crumb.fullPath}
							>
								{crumb.name}
							</button>
							{#if i < pathBreadcrumbs.length - 1}
								<ChevronRight class="w-3 h-3 text-slate-400 flex-shrink-0 opacity-60" />
							{/if}
						{/each}
					</div>

					<button
						onclick={() => (isEditingPath = true)}
						class="text-[11px] text-indigo-600 dark:text-indigo-400 hover:underline flex-shrink-0 font-medium px-1"
					>
						Edit
					</button>
				{:else}
					<form onsubmit={handleManualPathSubmit} class="flex-1 flex items-center gap-1.5">
						<input
							type="text"
							bind:value={manualPathInput}
							placeholder="/path/to/folder"
							class="flex-1 px-2.5 py-1 text-xs rounded-lg border border-indigo-400 bg-white dark:bg-slate-800 text-slate-800 dark:text-slate-100 font-mono outline-none"
						/>
						<button
							type="submit"
							class="px-2 py-1 text-xs rounded-lg bg-indigo-600 text-white font-semibold"
						>
							Go
						</button>
						<button
							type="button"
							onclick={() => (isEditingPath = false)}
							class="px-2 py-1 text-xs rounded-lg border border-slate-300 dark:border-slate-700 text-slate-600 dark:text-slate-300"
						>
							Cancel
						</button>
					</form>
				{/if}
			</div>

			<!-- Search / Filter in Current Folder -->
			<div class="px-4 py-2 border-b border-slate-100 dark:border-slate-800/40 flex items-center gap-2 flex-none">
				<Search class="w-3.5 h-3.5 text-slate-400" />
				<input
					type="text"
					bind:value={searchFilter}
					placeholder="Filter folders..."
					class="w-full text-xs bg-transparent border-none outline-none text-slate-700 dark:text-slate-200 placeholder-slate-400"
				/>
				{#if searchFilter}
					<button
						onclick={() => (searchFilter = '')}
						class="text-xs text-slate-400 hover:text-slate-600"
					>
						<X class="w-3.5 h-3.5" />
					</button>
				{/if}
			</div>

			<!-- Folder Content Listing -->
			<div class="flex-1 overflow-y-auto p-2 min-h-[220px] max-h-[360px] space-y-1">
				{#if isLoading}
					<div class="flex flex-col items-center justify-center h-48 gap-2 text-slate-400">
						<RefreshCw class="w-5 h-5 animate-spin text-indigo-500" />
						<span class="text-xs">Loading directory contents...</span>
					</div>
				{:else if errorMessage}
					<div class="flex flex-col items-center justify-center h-48 gap-2 px-6 text-center text-rose-500 dark:text-rose-400">
						<AlertCircle class="w-6 h-6 stroke-[2]" />
						<p class="text-xs font-semibold">{errorMessage}</p>
						<button
							onclick={() => loadPath(parentPath || undefined)}
							class="mt-2 px-3 py-1 text-xs rounded-lg bg-slate-100 dark:bg-slate-800 border border-slate-200 dark:border-slate-700 text-slate-700 dark:text-slate-300 hover:bg-slate-200"
						>
							Return to parent
						</button>
					</div>
				{:else if dirEntries.length === 0}
					<div class="flex flex-col items-center justify-center h-44 gap-1.5 text-slate-400 text-center">
						<Folder class="w-8 h-8 stroke-[1.5] text-slate-300 dark:text-slate-700" />
						<p class="text-xs font-medium text-slate-500">No subfolders found</p>
						<p class="text-[11px] text-slate-400">You can select this folder as the root to share.</p>
					</div>
				{:else}
					{#each dirEntries as entry}
						<button
							onclick={() => handleNavigate(entry.path)}
							class="w-full flex items-center justify-between p-2.5 rounded-xl hover:bg-slate-100 dark:hover:bg-slate-800/80 transition-all text-left cursor-pointer group"
						>
							<div class="flex items-center gap-2.5 min-w-0">
								<div class="p-1.5 rounded-lg bg-amber-500/10 text-amber-500 group-hover:scale-105 transition-transform flex-shrink-0">
									<Folder class="w-4 h-4 fill-amber-500/20" />
								</div>
								<span class="text-xs font-semibold text-slate-800 dark:text-slate-200 truncate">
									{entry.name}
								</span>
							</div>

							<div class="flex items-center gap-2 text-slate-400 flex-shrink-0">
								<ChevronRight class="w-4 h-4 group-hover:text-indigo-500 group-hover:translate-x-0.5 transition-all" />
							</div>
						</button>
					{/each}
				{/if}
			</div>

			<!-- Footer with Confirmation -->
			<div class="flex items-center justify-between gap-3 p-3.5 border-t border-slate-200 dark:border-slate-800 flex-none bg-slate-50 dark:bg-slate-950/70">
				<div class="min-w-0 flex-1">
					<p class="text-[10px] text-slate-400 uppercase tracking-wider font-semibold">Selected Directory</p>
					<p class="text-xs font-bold text-slate-900 dark:text-white truncate font-mono">
						{currentPath || 'None'}
					</p>
				</div>

				<div class="flex items-center gap-2 flex-shrink-0">
					<button
						onclick={onClose}
						class="px-3 py-2 rounded-xl border border-slate-200 dark:border-slate-800 bg-white dark:bg-slate-900 hover:bg-slate-100 dark:hover:bg-slate-800 text-slate-700 dark:text-slate-300 text-xs font-semibold transition-colors cursor-pointer"
					>
						Cancel
					</button>

					<button
						onclick={handleConfirm}
						disabled={!currentPath || isLoading}
						class="flex items-center gap-1.5 px-4 py-2 rounded-xl bg-indigo-600 hover:bg-indigo-500 disabled:opacity-50 text-white text-xs font-semibold shadow-md shadow-indigo-600/20 transition-all cursor-pointer"
					>
						<Check class="w-3.5 h-3.5 stroke-[2.5]" />
						<span>Select Folder</span>
					</button>
				</div>
			</div>
		</div>
	</div>
{/if}
