<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { appState } from '$lib/state.svelte';
	import { shortenFingerprint } from '$lib/format';
	import type { ChatConversationDto, ChatMessageViewDto } from '$lib/types';
	import {
		MessageSquare,
		Users,
		User,
		Plus,
		Send,
		Mic,
		Square,
		Paperclip,
		ShieldAlert,
		ShieldCheck,
		Check,
		CheckCheck,
		Clock,
		ArrowRightLeft,
		Play,
		Pause,
		Volume2,
		RefreshCw,
		FileText,
		Image as ImageIcon,
		Film,
		Search,
		X,
		ChevronLeft,
		Sparkles
	} from 'lucide-svelte';

	let messageInput = $state('');
	let conversationSearch = $state('');
	let isRecording = $state(false);
	let recordDurationSec = $state(0);
	let recordTimer: any = null;
	let recordedAudioBlob = $state<Blob | null>(null);
	let mediaRecorder: MediaRecorder | null = null;
	let audioChunks: Blob[] = [];
	let playingAudioId = $state<string | null>(null);
	let audioElement = $state<HTMLAudioElement | null>(null);
	let messagesContainer = $state<HTMLDivElement | null>(null);

	// New group dialog state
	let showNewGroupModal = $state(false);
	let newGroupName = $state('');
	let selectedGroupMembers = $state<string[]>([]);
	let isCreatingGroup = $state(false);

	// New 1:1 chat dialog state
	let showNewChatModal = $state(false);
	let newChatTarget = $state('');

	// Mobile view toggle: show list or show active conversation
	let mobileShowChat = $state(false);

	let filteredConversations = $derived.by(() => {
		let list = appState.conversations;
		if (!conversationSearch.trim()) return list;
		const q = conversationSearch.toLowerCase();
		return list.filter(
			(c) =>
				c.alias.toLowerCase().includes(q) ||
				c.targetFp.toLowerCase().includes(q) ||
				(c.lastMessage && c.lastMessage.toLowerCase().includes(q))
		);
	});

	let activeConv = $derived.by(() => appState.activeConversation);

	async function scrollToBottom() {
		await tick();
		if (messagesContainer) {
			messagesContainer.scrollTop = messagesContainer.scrollHeight;
		}
	}

	onMount(() => {
		appState.loadConversations();
		scrollToBottom();
	});

	$effect(() => {
		// When active conversation changes, scroll to bottom
		if (appState.activeConversationId && appState.activeMessages.length >= 0) {
			scrollToBottom();
		}
	});

	function handleSelectConversation(conv: ChatConversationDto) {
		appState.selectConversation(conv.id);
		mobileShowChat = true;
	}

	async function handleSendMessage() {
		if (!messageInput.trim()) return;
		const text = messageInput;
		messageInput = '';
		try {
			await appState.sendChatMessage(text, appState.isStrangerActive);
			scrollToBottom();
		} catch (e) {
			console.error('Failed to send:', e);
		}
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && !e.shiftKey) {
			e.preventDefault();
			handleSendMessage();
		}
	}

	// Microphone Voice Recorder
	async function startRecording() {
		try {
			audioChunks = [];
			recordDurationSec = 0;
			recordedAudioBlob = null;

			if (navigator.mediaDevices && navigator.mediaDevices.getUserMedia) {
				const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
				mediaRecorder = new MediaRecorder(stream);
				mediaRecorder.ondataavailable = (event) => {
					if (event.data.size > 0) audioChunks.push(event.data);
				};
				mediaRecorder.onstop = () => {
					const blob = new Blob(audioChunks, { type: 'audio/webm' });
					recordedAudioBlob = blob;
					stream.getTracks().forEach((track) => track.stop());
				};
				mediaRecorder.start();
				isRecording = true;
				recordTimer = setInterval(() => {
					recordDurationSec += 1;
				}, 1000);
			} else {
				// Fallback simulation for unsupported environments
				isRecording = true;
				recordTimer = setInterval(() => {
					recordDurationSec += 1;
				}, 1000);
			}
		} catch (err) {
			console.warn('Microphone permission not granted or unavailable, starting simulated recorder:', err);
			isRecording = true;
			recordTimer = setInterval(() => {
				recordDurationSec += 1;
			}, 1000);
		}
	}

	function stopRecording(sendDirectly = true) {
		if (!isRecording) return;
		isRecording = false;
		if (recordTimer) {
			clearInterval(recordTimer);
			recordTimer = null;
		}

		if (mediaRecorder && mediaRecorder.state !== 'inactive') {
			mediaRecorder.stop();
		}

		if (sendDirectly && recordDurationSec > 0) {
			const simulatedVoiceText = `[Voice: ${recordDurationSec}s]`;
			appState.sendChatMessage(simulatedVoiceText, appState.isStrangerActive);
			recordDurationSec = 0;
			recordedAudioBlob = null;
		}
	}

	function cancelRecording() {
		if (isRecording) {
			isRecording = false;
			if (recordTimer) {
				clearInterval(recordTimer);
				recordTimer = null;
			}
			if (mediaRecorder && mediaRecorder.state !== 'inactive') {
				mediaRecorder.stop();
			}
			recordDurationSec = 0;
			audioChunks = [];
			recordedAudioBlob = null;
		}
	}

	function playVoiceNote(msg: ChatMessageViewDto) {
		if (playingAudioId === msg.id) {
			if (audioElement) {
				audioElement.pause();
			}
			playingAudioId = null;
		} else {
			playingAudioId = msg.id;
			// Simulated playback timeout for voice notes
			const duration = (msg.voiceDurationSec || 4) * 1000;
			setTimeout(() => {
				if (playingAudioId === msg.id) {
					playingAudioId = null;
				}
			}, duration);
		}
	}

	function handleApproveStranger() {
		if (activeConv) {
			appState.respondTrust(activeConv.targetFp, true);
		}
	}

	function handleBlockStranger() {
		if (activeConv) {
			appState.respondTrust(activeConv.targetFp, false);
		}
	}

	async function handleCreateGroup() {
		if (!newGroupName.trim() || selectedGroupMembers.length === 0) return;
		isCreatingGroup = true;
		try {
			await appState.createChatGroup(newGroupName.trim(), selectedGroupMembers);
			showNewGroupModal = false;
			newGroupName = '';
			selectedGroupMembers = [];
		} catch (e) {
			console.error('Group creation failed:', e);
		} finally {
			isCreatingGroup = false;
		}
	}

	function toggleGroupMember(fp: string) {
		if (selectedGroupMembers.includes(fp)) {
			selectedGroupMembers = selectedGroupMembers.filter((x) => x !== fp);
		} else {
			selectedGroupMembers.push(fp);
		}
	}

	function handleStartNewChat() {
		if (!newChatTarget.trim()) return;
		const target = newChatTarget.trim();
		// Create or select conversation
		appState.selectConversation(target);
		showNewChatModal = false;
		newChatTarget = '';
		mobileShowChat = true;
	}

	function formatTime(timestampMs: number): string {
		if (!timestampMs) return '';
		const d = new Date(timestampMs);
		return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
	}
</script>

<div class="h-full w-full flex overflow-hidden bg-slate-50 dark:bg-slate-950">
	<!-- Left Conversations List Panel (Desktop always visible, Mobile conditionally visible) -->
	<section
		aria-label="Chat conversations"
		class="{mobileShowChat ? 'hidden md:flex' : 'flex'} w-full md:w-80 flex-col border-r border-slate-200 dark:border-slate-800/80 bg-white dark:bg-slate-900/60 flex-none"
	>
		<!-- Conversations Header -->
		<div class="p-3.5 border-b border-slate-200 dark:border-slate-800/80 space-y-2.5">
			<div class="flex items-center justify-between">
				<div class="flex items-center gap-2">
					<div class="p-1.5 rounded-lg bg-indigo-50 dark:bg-indigo-500/15 text-indigo-600 dark:text-indigo-400">
						<MessageSquare class="w-4 h-4" />
					</div>
					<h2 class="font-bold text-sm text-slate-900 dark:text-white">Encrypted Chat</h2>
				</div>
				<div class="flex items-center gap-1">
					<button
						onclick={() => (showNewGroupModal = true)}
						class="p-1.5 rounded-lg text-slate-500 hover:text-indigo-600 hover:bg-slate-100 dark:hover:bg-slate-800 transition-colors"
						title="New Decentralized Group"
					>
						<Users class="w-4 h-4" />
					</button>
					<button
						onclick={() => (showNewChatModal = true)}
						class="p-1.5 rounded-lg text-slate-500 hover:text-indigo-600 hover:bg-slate-100 dark:hover:bg-slate-800 transition-colors"
						title="New 1:1 Conversation"
					>
						<Plus class="w-4 h-4" />
					</button>
				</div>
			</div>

			<!-- Search Bar -->
			<div class="relative">
				<Search class="w-3.5 h-3.5 text-slate-400 absolute left-2.5 top-1/2 -translate-y-1/2" />
				<input
					type="text"
					bind:value={conversationSearch}
					placeholder="Search conversations..."
					class="w-full pl-8 pr-3 py-1.5 text-xs rounded-lg bg-slate-100 dark:bg-slate-800/80 border border-slate-200 dark:border-slate-700/60 text-slate-800 dark:text-slate-100 placeholder-slate-400 focus:outline-hidden focus:border-indigo-500"
				/>
			</div>
		</div>

		<!-- Conversations Scroll Area -->
		<div class="flex-1 overflow-y-auto divide-y divide-slate-100 dark:divide-slate-800/40">
			{#if filteredConversations.length === 0}
				<div class="p-6 text-center text-slate-400 dark:text-slate-500 text-xs space-y-2">
					<MessageSquare class="w-8 h-8 mx-auto opacity-40" />
					<p>No conversations yet</p>
					<p class="text-[11px] text-slate-400">Click "+" to start a secure chat with a local peer.</p>
				</div>
			{:else}
				{#each filteredConversations as conv (conv.id)}
					<button
						onclick={() => handleSelectConversation(conv)}
						class="w-full text-left p-3 flex items-start gap-3 transition-colors hover:bg-slate-50 dark:hover:bg-slate-800/40 {appState.activeConversationId === conv.id ? 'bg-indigo-50/70 dark:bg-indigo-600/15 border-l-3 border-indigo-600 dark:border-indigo-500' : ''}"
					>
						<!-- Avatar / Icon -->
						<div class="relative flex-none">
							<div class="w-10 h-10 rounded-xl flex items-center justify-center font-bold text-xs {conv.isGroup ? 'bg-gradient-to-br from-indigo-500 to-purple-600 text-white' : 'bg-gradient-to-br from-sky-400 to-blue-600 text-white'}">
								{#if conv.isGroup}
									<Users class="w-5 h-5" />
								{:else}
									<User class="w-5 h-5" />
								{/if}
							</div>
							{#if conv.trustState === 'Stranger' || conv.trustState === 'PendingApproval'}
								<span class="absolute -bottom-1 -right-1 w-3.5 h-3.5 rounded-full bg-amber-500 border-2 border-white dark:border-slate-900 flex items-center justify-center text-[8px] text-white" title="Stranger / Pending Approval">
									!
								</span>
							{/if}
						</div>

						<!-- Info -->
						<div class="flex-1 min-w-0">
							<div class="flex items-center justify-between mb-0.5">
								<h3 class="font-semibold text-xs text-slate-900 dark:text-white truncate">
									{conv.alias}
								</h3>
								<span class="text-[10px] text-slate-400 flex-none">
									{formatTime(conv.lastMessageTimeMs)}
								</span>
							</div>

							<div class="flex items-center justify-between gap-1">
								<p class="text-[11px] text-slate-500 dark:text-slate-400 truncate">
									{conv.lastMessage || 'No messages yet'}
								</p>
								{#if conv.unreadCount > 0}
									<span class="px-1.5 py-0.2 rounded-full text-[9px] font-bold bg-indigo-600 text-white flex-none">
										{conv.unreadCount}
									</span>
								{/if}
							</div>

							<div class="flex items-center gap-1.5 mt-1">
								{#if conv.isGroup}
									<span class="text-[9px] px-1 py-0.2 rounded bg-indigo-50 dark:bg-indigo-950/60 text-indigo-600 dark:text-indigo-400 font-mono">
										Causal DAG
									</span>
								{:else if conv.trustState === 'Stranger'}
									<span class="text-[9px] px-1 py-0.2 rounded bg-amber-50 dark:bg-amber-950/60 text-amber-600 dark:text-amber-400 font-medium">
										Stranger
									</span>
								{:else if conv.trustState === 'PendingApproval'}
									<span class="text-[9px] px-1 py-0.2 rounded bg-amber-50 dark:bg-amber-950/60 text-amber-600 dark:text-amber-400 font-medium">
										Pending Approval
									</span>
								{:else}
									<span class="text-[9px] px-1 py-0.2 rounded bg-emerald-50 dark:bg-emerald-950/60 text-emerald-600 dark:text-emerald-400 font-medium">
										Trusted E2EE
									</span>
								{/if}
							</div>
						</div>
					</button>
				{/each}
			{/if}
		</div>
	</section>

	<!-- Right Chat Main Canvas (Desktop always visible, Mobile conditionally visible) -->
	<section
		aria-label="Chat messages"
		class="{mobileShowChat ? 'flex' : 'hidden md:flex'} flex-1 flex-col h-full overflow-hidden bg-slate-50 dark:bg-slate-950"
	>
		{#if activeConv}
			<!-- Chat Header -->
			<div class="px-4 py-3 bg-white dark:bg-slate-900/80 border-b border-slate-200 dark:border-slate-800/80 flex items-center justify-between flex-none">
				<div class="flex items-center gap-3">
					<button
						onclick={() => (mobileShowChat = false)}
						class="md:hidden p-1.5 -ml-1 text-slate-500 hover:text-slate-800 dark:hover:text-white"
						title="Back to conversation list"
					>
						<ChevronLeft class="w-5 h-5" />
					</button>

					<div class="w-9 h-9 rounded-xl flex items-center justify-center font-bold text-xs {activeConv.isGroup ? 'bg-indigo-600 text-white' : 'bg-sky-500 text-white'}">
						{#if activeConv.isGroup}
							<Users class="w-4 h-4" />
						{:else}
							<User class="w-4 h-4" />
						{/if}
					</div>

					<div>
						<div class="flex items-center gap-2">
							<h3 class="font-bold text-xs sm:text-sm text-slate-900 dark:text-white">
								{activeConv.alias}
							</h3>
							{#if activeConv.isGroup}
								<span class="text-[9px] px-1.5 py-0.2 rounded-full font-mono bg-indigo-100 dark:bg-indigo-900/60 text-indigo-700 dark:text-indigo-300">
									Group
								</span>
							{:else}
								<span class="text-[9px] px-1.5 py-0.2 rounded-full font-mono bg-slate-100 dark:bg-slate-800 text-slate-600 dark:text-slate-400">
									{shortenFingerprint(activeConv.targetFp)}
								</span>
							{/if}
						</div>
						<p class="text-[10px] text-slate-400 font-mono">
							{#if activeConv.isGroup}
								Anti-Entropy Merkle-DAG • Pairwise E2EE
							{:else if activeConv.trustState === 'Stranger' || activeConv.trustState === 'PendingApproval'}
								Stranger Handshake Gate Active (Hashcash PoW verified)
							{:else}
								End-to-End Encrypted via ChaCha20-Poly1305
							{/if}
						</p>
					</div>
				</div>

				<div class="flex items-center gap-1.5">
					{#if activeConv.isGroup}
						<button
							onclick={() => appState.syncChatGroup(activeConv?.id || '', activeConv?.targetFp || '')}
							class="px-2 py-1 text-xs font-medium rounded-lg bg-indigo-50 dark:bg-indigo-950/60 text-indigo-600 dark:text-indigo-400 hover:bg-indigo-100 transition-colors flex items-center gap-1"
							title="Synchronize Causal DAG Frontier"
						>
							<RefreshCw class="w-3.5 h-3.5" />
							<span class="hidden sm:inline">Sync DAG</span>
						</button>
					{/if}
				</div>
			</div>

			<!-- Stranger Request Approval Banner (when peer is stranger or pending approval) -->
			{#if appState.isStrangerActive}
				<div class="p-3 bg-amber-50 dark:bg-amber-950/40 border-b border-amber-200 dark:border-amber-900/60 flex items-center justify-between gap-3 text-amber-900 dark:text-amber-200">
					<div class="flex items-center gap-2.5">
						<ShieldAlert class="w-5 h-5 text-amber-600 dark:text-amber-400 flex-none" />
						<div class="text-xs">
							<span class="font-semibold">{activeConv.alias}</span> is an untrusted stranger on the LAN.
							<span class="text-amber-700 dark:text-amber-300 block text-[11px]">
								Only text introductions (≤ 280 chars) with Hashcash PoW are allowed until approved.
							</span>
						</div>
					</div>
					<div class="flex items-center gap-2 flex-none">
						<button
							onclick={handleApproveStranger}
							class="px-2.5 py-1 text-xs font-semibold rounded-lg bg-emerald-600 hover:bg-emerald-700 text-white transition-colors flex items-center gap-1 cursor-pointer"
						>
							<ShieldCheck class="w-3.5 h-3.5" />
							<span>Approve</span>
						</button>
						<button
							onclick={handleBlockStranger}
							class="px-2 py-1 text-xs font-medium rounded-lg bg-red-100 dark:bg-red-950/60 text-red-700 dark:text-red-400 hover:bg-red-200 transition-colors cursor-pointer"
						>
							Block
						</button>
					</div>
				</div>
			{/if}

			<!-- Messages Timeline Area -->
			<div
				bind:this={messagesContainer}
				class="flex-1 overflow-y-auto p-4 space-y-3"
			>
				{#if appState.isChatLoading}
					<div class="flex justify-center p-8">
						<RefreshCw class="w-6 h-6 text-indigo-500 animate-spin" />
					</div>
				{:else if appState.activeMessages.length === 0}
					<div class="flex flex-col items-center justify-center h-full text-center text-slate-400 text-xs space-y-2">
						<Sparkles class="w-8 h-8 opacity-40 text-indigo-500" />
						<p class="font-semibold text-slate-600 dark:text-slate-300">End-to-End Encrypted Channel</p>
						<p class="max-w-xs text-[11px]">Messages and voice notes stream directly over LAN or multi-hop overlay mesh with monotonic delivery receipts.</p>
					</div>
				{:else}
					{#each appState.activeMessages as msg (msg.id)}
						<div class="flex flex-col {msg.isSelf ? 'items-end' : 'items-start'}">
							<div class="max-w-[85%] sm:max-w-md rounded-2xl px-3.5 py-2.5 shadow-xs transition-all {msg.isSelf ? 'bg-indigo-600 text-white rounded-br-xs' : 'bg-white dark:bg-slate-800 text-slate-800 dark:text-slate-100 border border-slate-200/80 dark:border-slate-700/60 rounded-bl-xs'}">
								<!-- Sender Label for Groups -->
								{#if activeConv.isGroup && !msg.isSelf}
									<p class="text-[10px] font-mono text-indigo-400 dark:text-indigo-300 mb-1">
										{shortenFingerprint(msg.senderFp)}
									</p>
								{/if}

								<!-- Content Renderers -->
								{#if msg.content.startsWith('[Voice:') || msg.mediaType === 'voice'}
									<!-- Voice Note Player -->
									<div class="flex items-center gap-3 py-1">
										<button
											onclick={() => playVoiceNote(msg)}
											class="w-8 h-8 rounded-full flex items-center justify-center {msg.isSelf ? 'bg-white/20 text-white hover:bg-white/30' : 'bg-indigo-100 dark:bg-indigo-900 text-indigo-600 dark:text-indigo-300 hover:bg-indigo-200'} transition-colors"
											title="Play voice note"
										>
											{#if playingAudioId === msg.id}
												<Pause class="w-4 h-4 fill-current" />
											{:else}
												<Play class="w-4 h-4 fill-current ml-0.5" />
											{/if}
										</button>
										<div class="space-y-1">
											<div class="flex items-center gap-1 h-4">
												<!-- Sound Waveform Bars -->
												{#each [3, 8, 14, 6, 12, 16, 9, 13, 5, 11] as h}
													<span
														class="w-1 rounded-full {msg.isSelf ? 'bg-white/70' : 'bg-indigo-500'} {playingAudioId === msg.id ? 'animate-pulse' : ''}"
														style="height: {h}px"
													></span>
												{/each}
											</div>
											<span class="text-[10px] opacity-80 font-mono">
												{msg.voiceDurationSec || 4}s Voice Note
											</span>
										</div>
									</div>
								{:else if msg.content.startsWith('[Image:') || msg.mediaType === 'image'}
									<!-- Image Attachment Preview -->
									<div class="space-y-1.5">
										<div class="rounded-xl overflow-hidden bg-slate-900/10 dark:bg-slate-950/40 p-2 flex items-center gap-2">
											<ImageIcon class="w-5 h-5 text-indigo-400" />
											<span class="text-xs font-medium">{msg.fileName || 'Image Attachment'}</span>
										</div>
										<p class="text-xs">{msg.content}</p>
									</div>
								{:else if msg.content.startsWith('[File:') || msg.mediaType === 'file'}
									<!-- Resumable File Attachment Card -->
									<div class="space-y-2 p-2 rounded-xl bg-black/5 dark:bg-white/5 border border-white/10">
										<div class="flex items-center gap-2">
											<FileText class="w-5 h-5 text-indigo-400 flex-none" />
											<div class="min-w-0 flex-1">
												<p class="text-xs font-medium truncate">{msg.fileName || 'Shared Document'}</p>
												<span class="text-[10px] opacity-75">{msg.fileSize ? `${Math.round(msg.fileSize / 1024)} KB` : 'Resumable Transfer'}</span>
											</div>
										</div>
										<!-- Resumable Progress Bar -->
										{#if msg.bytesTransferred && msg.fileSize && msg.bytesTransferred < msg.fileSize}
											<div class="w-full bg-black/20 rounded-full h-1.5 overflow-hidden">
												<div
													class="bg-indigo-400 h-full rounded-full transition-all"
													style="width: {(msg.bytesTransferred / msg.fileSize) * 100}%"
												></div>
											</div>
											<span class="text-[9px] opacity-70">
												Resuming from offset {Math.round(msg.bytesTransferred / 1024)} KB
											</span>
										{/if}
									</div>
								{:else}
									<!-- Plain Text Message -->
									<p class="text-xs sm:text-sm whitespace-pre-wrap break-words leading-relaxed">
										{msg.content}
									</p>
								{/if}

								<!-- Timestamp and Delivery State Checkmarks -->
								<div class="flex items-center justify-end gap-1.5 mt-1 text-[9px] opacity-75">
									<span>{formatTime(msg.timestampMs)}</span>
									{#if msg.isSelf}
										{#if msg.deliveryState === 'Queued'}
											<span title="Delay-Tolerant Outbox: Queued for Peer Reconnect">
												<Clock class="w-3 h-3" />
											</span>
										{:else if msg.deliveryState === 'Relayed'}
											<span title="Relayed: Deposited in Custodial Mailbox on Mesh Relay">
												<ArrowRightLeft class="w-3 h-3" />
											</span>
										{:else if msg.deliveryState === 'Delivered'}
											<span title="Delivered to Remote Endpoint">
												<CheckCheck class="w-3 h-3" />
											</span>
										{:else if msg.deliveryState === 'Read'}
											<span class="text-sky-300 dark:text-sky-400 font-bold" title="Read by Counter-party">
												<CheckCheck class="w-3 h-3" />
											</span>
										{:else}
											<span title="Sent">
												<Check class="w-3 h-3" />
											</span>
										{/if}
									{/if}
								</div>
							</div>
						</div>
					{/each}
				{/if}
			</div>

			<!-- Input / Voice Recorder Area -->
			<div class="p-3 bg-white dark:bg-slate-900 border-t border-slate-200 dark:border-slate-800/80 flex-none">
				{#if isRecording}
					<!-- Live Voice Recording Bar -->
					<div class="flex items-center justify-between gap-3 p-2 rounded-xl bg-red-50 dark:bg-red-950/40 border border-red-200 dark:border-red-900/60 animate-pulse">
						<div class="flex items-center gap-2 text-red-600 dark:text-red-400">
							<span class="w-2.5 h-2.5 rounded-full bg-red-600 animate-ping"></span>
							<Mic class="w-4 h-4" />
							<span class="text-xs font-semibold font-mono">
								Recording Voice Note: {Math.floor(recordDurationSec / 60).toString().padStart(2, '0')}:{(recordDurationSec % 60).toString().padStart(2, '0')}
							</span>
						</div>
						<div class="flex items-center gap-2">
							<button
								onclick={cancelRecording}
								class="px-2.5 py-1 text-xs rounded-lg bg-slate-200 dark:bg-slate-800 text-slate-700 dark:text-slate-300 hover:bg-slate-300"
							>
								Cancel
							</button>
							<button
								onclick={() => stopRecording(true)}
								class="px-3 py-1 text-xs font-bold rounded-lg bg-red-600 hover:bg-red-700 text-white flex items-center gap-1"
							>
								<Send class="w-3 h-3" />
								<span>Send</span>
							</button>
						</div>
					</div>
				{:else}
					<!-- Message Composition Bar -->
					<div class="flex items-center gap-2">
						<button
							onclick={() => {
								if (appState.isStrangerActive) {
									alert('Stranger handshakes only permit plain-text introduction. Approve contact to send media attachments.');
									return;
								}
								appState.sendMode = 'single';
								appState.showSendDialog = true;
							}}
							class="p-2 rounded-lg text-slate-500 hover:text-indigo-600 hover:bg-slate-100 dark:hover:bg-slate-800 transition-colors"
							title={appState.isStrangerActive ? 'Attachments locked for strangers' : 'Attach file or media'}
						>
							<Paperclip class="w-4 h-4" />
						</button>

						<button
							onclick={startRecording}
							class="p-2 rounded-lg text-slate-500 hover:text-red-500 hover:bg-slate-100 dark:hover:bg-slate-800 transition-colors"
							title="Hold or click to record voice note"
						>
							<Mic class="w-4 h-4" />
						</button>

						<textarea
							rows="1"
							bind:value={messageInput}
							onkeydown={handleKeydown}
							placeholder={appState.isStrangerActive ? "Stranger Introduction (max 280 chars, plain text only)..." : "Write an encrypted message..."}
							maxlength={appState.isStrangerActive ? 280 : 65535}
							class="flex-1 py-2 px-3 text-xs sm:text-sm rounded-xl bg-slate-100 dark:bg-slate-800 border border-slate-200 dark:border-slate-700 text-slate-900 dark:text-white placeholder-slate-400 focus:outline-hidden focus:border-indigo-500 resize-none max-h-24"
						></textarea>

						<button
							onclick={handleSendMessage}
							disabled={!messageInput.trim()}
							class="p-2 sm:px-3.5 sm:py-2 rounded-xl bg-indigo-600 hover:bg-indigo-700 disabled:opacity-40 text-white font-medium text-xs flex items-center gap-1.5 transition-colors cursor-pointer"
							title="Send message"
						>
							<Send class="w-4 h-4" />
							<span class="hidden sm:inline">Send</span>
						</button>
					</div>
				{/if}
			</div>
		{:else}
			<!-- Empty State when no conversation is selected -->
			<div class="flex-1 flex flex-col items-center justify-center text-center p-6 text-slate-400 space-y-3">
				<div class="p-4 rounded-2xl bg-indigo-50 dark:bg-indigo-950/40 text-indigo-500">
					<MessageSquare class="w-10 h-10" />
				</div>
				<h3 class="font-bold text-slate-700 dark:text-slate-200 text-sm">Select a Conversation</h3>
				<p class="text-xs max-w-xs text-slate-400">
					Pick a peer from the left or click "+" to initiate a secure encrypted chat or decentralized group.
				</p>
			</div>
		{/if}
	</section>
</div>

<!-- Modal: New Decentralized Group -->
{#if showNewGroupModal}
	<div class="fixed inset-0 z-50 flex items-center justify-center p-4 bg-slate-950/60 backdrop-blur-xs">
		<div class="bg-white dark:bg-slate-900 rounded-2xl max-w-sm w-full p-5 space-y-4 shadow-xl border border-slate-200 dark:border-slate-800">
			<div class="flex items-center justify-between">
				<h3 class="font-bold text-sm text-slate-900 dark:text-white flex items-center gap-2">
					<Users class="w-4 h-4 text-indigo-600" />
					<span>Create Causal Group Chat</span>
				</h3>
				<button onclick={() => (showNewGroupModal = false)} class="p-1 text-slate-400 hover:text-slate-600">
					<X class="w-4 h-4" />
				</button>
			</div>

			<div class="space-y-3">
				<div>
					<label for="group-name-input" class="block text-xs font-medium text-slate-600 dark:text-slate-400 mb-1">Group Name</label>
					<input
						id="group-name-input"
						type="text"
						bind:value={newGroupName}
						placeholder="e.g. Project LAN Team"
						class="w-full px-3 py-1.5 text-xs rounded-lg bg-slate-100 dark:bg-slate-800 border border-slate-200 dark:border-slate-700 text-slate-900 dark:text-white"
					/>
				</div>

				<div>
					<span class="block text-xs font-medium text-slate-600 dark:text-slate-400 mb-1">Select Local Peers</span>
					<div class="max-h-36 overflow-y-auto space-y-1 border border-slate-200 dark:border-slate-800 rounded-lg p-2">
						{#if appState.peers.length === 0}
							<p class="text-[11px] text-slate-400 text-center py-2">No peers discovered on LAN yet.</p>
						{:else}
							{#each appState.peers as p}
								<label class="flex items-center gap-2 p-1.5 rounded hover:bg-slate-100 dark:hover:bg-slate-800 text-xs cursor-pointer">
									<input
										type="checkbox"
										checked={selectedGroupMembers.includes(p.fingerprint)}
										onchange={() => toggleGroupMember(p.fingerprint)}
										class="rounded text-indigo-600"
									/>
									<span class="font-medium text-slate-800 dark:text-slate-200 truncate">{p.alias}</span>
									<span class="text-[10px] text-slate-400 font-mono ml-auto">{shortenFingerprint(p.fingerprint)}</span>
								</label>
							{/each}
						{/if}
					</div>
				</div>
			</div>

			<div class="flex items-center justify-end gap-2 pt-2 border-t border-slate-100 dark:border-slate-800">
				<button
					onclick={() => (showNewGroupModal = false)}
					class="px-3 py-1.5 text-xs rounded-lg text-slate-600 hover:bg-slate-100 dark:hover:bg-slate-800"
				>
					Cancel
				</button>
				<button
					onclick={handleCreateGroup}
					disabled={!newGroupName.trim() || selectedGroupMembers.length === 0 || isCreatingGroup}
					class="px-3.5 py-1.5 text-xs font-bold rounded-lg bg-indigo-600 hover:bg-indigo-700 disabled:opacity-40 text-white"
				>
					{isCreatingGroup ? 'Creating...' : 'Create Group'}
				</button>
			</div>
		</div>
	</div>
{/if}

<!-- Modal: New 1:1 Chat -->
{#if showNewChatModal}
	<div class="fixed inset-0 z-50 flex items-center justify-center p-4 bg-slate-950/60 backdrop-blur-xs">
		<div class="bg-white dark:bg-slate-900 rounded-2xl max-w-sm w-full p-5 space-y-4 shadow-xl border border-slate-200 dark:border-slate-800">
			<div class="flex items-center justify-between">
				<h3 class="font-bold text-sm text-slate-900 dark:text-white flex items-center gap-2">
					<User class="w-4 h-4 text-indigo-600" />
					<span>New 1:1 Conversation</span>
				</h3>
				<button onclick={() => (showNewChatModal = false)} class="p-1 text-slate-400 hover:text-slate-600">
					<X class="w-4 h-4" />
				</button>
			</div>

			<div class="space-y-3">
				<div>
					<label for="peer-fingerprint-alias-input" class="block text-xs font-medium text-slate-600 dark:text-slate-400 mb-1">Peer Fingerprint or Discovered Peer</label>
					<input
						id="peer-fingerprint-alias-input"
						type="text"
						bind:value={newChatTarget}
						placeholder="Paste 64-char fingerprint or alias..."
						class="w-full px-3 py-1.5 text-xs rounded-lg bg-slate-100 dark:bg-slate-800 border border-slate-200 dark:border-slate-700 text-slate-900 dark:text-white"
					/>
				</div>

				{#if appState.peers.length > 0}
					<div>
						<span class="block text-xs font-medium text-slate-600 dark:text-slate-400 mb-1">Or choose active peer:</span>
						<div class="max-h-32 overflow-y-auto space-y-1 border border-slate-200 dark:border-slate-800 rounded-lg p-1.5">
							{#each appState.peers as p}
								<button
									onclick={() => {
										newChatTarget = p.fingerprint;
										handleStartNewChat();
									}}
									class="w-full text-left p-1.5 rounded hover:bg-slate-100 dark:hover:bg-slate-800 text-xs flex items-center justify-between"
								>
									<span class="font-medium text-slate-800 dark:text-slate-200 truncate">{p.alias}</span>
									<span class="text-[10px] text-slate-400 font-mono">{shortenFingerprint(p.fingerprint)}</span>
								</button>
							{/each}
						</div>
					</div>
				{/if}
			</div>

			<div class="flex items-center justify-end gap-2 pt-2 border-t border-slate-100 dark:border-slate-800">
				<button
					onclick={() => (showNewChatModal = false)}
					class="px-3 py-1.5 text-xs rounded-lg text-slate-600 hover:bg-slate-100 dark:hover:bg-slate-800"
				>
					Cancel
				</button>
				<button
					onclick={handleStartNewChat}
					disabled={!newChatTarget.trim()}
					class="px-3.5 py-1.5 text-xs font-bold rounded-lg bg-indigo-600 hover:bg-indigo-700 disabled:opacity-40 text-white"
				>
					Start Chat
				</button>
			</div>
		</div>
	</div>
{/if}
