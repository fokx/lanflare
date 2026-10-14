<script lang="ts">
	import { appState } from '$lib/state.svelte';
	import { themeManager, type ThemeMode } from '$lib/theme.svelte';
	import { updateConfig } from '$lib/api';
	import { Settings, Folder, KeyRound, Radio, Shield, Check, X, Palette, Sun, Moon, Monitor } from 'lucide-svelte';

	let alias = $state(appState.status?.alias || '');
	let port = $state(appState.status?.port || 53317);
	let downloadDir = $state(appState.status?.downloadDir || '');
	let autoAccept = $state(appState.status?.autoAccept || false);
	let pin = $state('');
	let pinEnabled = $state(appState.status?.pinEnabled || false);
	let isSaving = $state(false);
	let saveSuccess = $state(false);

	$effect(() => {
		if (appState.status) {
			alias = appState.status.alias;
			port = appState.status.port;
			downloadDir = appState.status.downloadDir;
			autoAccept = appState.status.autoAccept;
			pinEnabled = appState.status.pinEnabled;
		}
	});

	async function handleSave() {
		isSaving = true;
		saveSuccess = false;
		try {
			const updated = await updateConfig({
				alias,
				port,
				downloadDir,
				autoAccept,
				pin: pinEnabled ? (pin ? pin : undefined) : null
			});
			appState.status = updated;
			saveSuccess = true;
			setTimeout(() => {
				saveSuccess = false;
				appState.showSettings = false;
			}, 600);
		} catch (e) {
			console.error('Failed to save settings:', e);
		} finally {
			isSaving = false;
		}
	}
</script>

{#if appState.showSettings}
	<div
		class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 dark:bg-black/75 backdrop-blur-sm p-4 animate-in fade-in duration-150"
	>
		<div
			class="w-full max-w-lg bg-white dark:bg-slate-900 border border-slate-200 dark:border-slate-800 rounded-2xl shadow-2xl p-6 space-y-6 animate-in zoom-in-95 duration-150"
		>
			<!-- Header -->
			<div class="flex items-center justify-between border-b border-slate-200 dark:border-slate-800 pb-4">
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

			<!-- Settings Form -->
			<div class="space-y-4 text-sm">
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
							<span>Auto (System)</span>
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
					<p class="text-[11px] text-slate-400 dark:text-slate-500">
						Auto detects system dark mode and defaults to light theme if detection fails.
					</p>
				</div>

				<!-- Device Alias -->
				<div class="space-y-1.5">
					<label for="alias-input" class="text-xs font-medium text-slate-700 dark:text-slate-300">Device Alias (Name on Network)</label>
					<input
						id="alias-input"
						type="text"
						bind:value={alias}
						class="w-full bg-slate-50 dark:bg-slate-950 border border-slate-300 dark:border-slate-800 rounded-lg px-3 py-2 text-sm text-slate-900 dark:text-slate-200 focus:outline-none focus:border-indigo-500"
					/>
				</div>

				<!-- Server Port & Fingerprint -->
				<div class="grid grid-cols-1 sm:grid-cols-2 gap-3 sm:gap-4">
					<div class="space-y-1.5">
						<label for="port-input" class="text-xs font-medium text-slate-700 dark:text-slate-300">Port (Default: 53317)</label>
						<input
							id="port-input"
							type="number"
							bind:value={port}
							class="w-full bg-slate-50 dark:bg-slate-950 border border-slate-300 dark:border-slate-800 rounded-lg px-3 py-2 text-sm text-slate-900 dark:text-slate-200 focus:outline-none focus:border-indigo-500 font-mono"
						/>
					</div>

					<div class="space-y-1.5">
						<span class="text-xs font-medium text-slate-700 dark:text-slate-300 block">TLS Fingerprint</span>
						<div
							class="w-full bg-slate-50 dark:bg-slate-950 border border-slate-200 dark:border-slate-800/80 rounded-lg px-3 py-2 text-xs text-slate-500 dark:text-slate-400 font-mono truncate cursor-help"
							title={appState.status?.fingerprint || ''}
						>
							{appState.status?.fingerprint ? appState.status.fingerprint.slice(0, 16) + '...' : '--'}
						</div>
					</div>
				</div>

				<!-- Download Directory -->
				<div class="space-y-1.5">
					<label for="download-input" class="text-xs font-medium text-slate-700 dark:text-slate-300">Download Directory</label>
					<div class="flex items-center gap-2">
						<input
							id="download-input"
							type="text"
							bind:value={downloadDir}
							class="flex-1 bg-slate-50 dark:bg-slate-950 border border-slate-300 dark:border-slate-800 rounded-lg px-3 py-2 text-sm text-slate-900 dark:text-slate-200 focus:outline-none focus:border-indigo-500 font-mono"
						/>
					</div>
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
			</div>

			<!-- Footer Buttons -->
			<div class="flex items-center justify-end gap-3 pt-2 border-t border-slate-200 dark:border-slate-800">
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
