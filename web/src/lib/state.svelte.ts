import type {
	NodeStatusDto,
	DiscoveredPeer,
	IncomingTransferEventPayload,
	TransferProgressPayload,
	OutgoingTransferInitiatedPayload,
	TransferQueueItem,
	FileReceivedPayload,
	SendMode,
	PreparedSendItem,
	BrowserDownloadRequest,
	MeshTopologyView,
	StartSimulationPayload,
	ToggleMeshLinkPayload,
	RunTraceroutePayload,
	MeshTracerouteResponse,
	RunBenchmarkPayload,
	SpeedtestResultDto
} from './types';
import {
	getNodeStatus,
	getDiscoveredPeers,
	removeDiscoveredPeer,
	startDiscovery,
	announcePresence,
	startReceiverServer,
	tauriListen,
	respondIncomingTransfer,
	cancelOutgoingTransfer,
	createTempTextFile,
	offerDownloadFiles,
	clearDownloadFiles,
	getTransferHistory,
	saveTransferHistory,
	readClipboard,
	saveClipboardImage,
	isTauri,
	sendFiles,
	respondBrowserDownload,
	getMeshTopology,
	startMeshSimulation,
	toggleMeshLink,
	runMeshTraceroute,
	runMeshBenchmark
} from './api';
import { readText as tauriReadText, readImage as tauriReadImage } from '@tauri-apps/plugin-clipboard-manager';

export class AppState {
	// Node Status
	status = $state<NodeStatusDto | null>(null);
	serverPort = $state<number | null>(null);
	portConflict = $state<boolean>(false);
	isDiscovering = $state<boolean>(false);

	// Discovered Peers
	peers = $state<DiscoveredPeer[]>([]);
	searchQuery = $state<string>('');

	// Send Mode: 'single' | 'multiple' | 'link'
	sendMode = $state<SendMode>('single');

	// Multi-Recipient Selection (Peer fingerprints)
	selectedRecipients = $state<string[]>([]);

	// Prepared items to send (files, folders, text messages)
	preparedItems = $state<PreparedSendItem[]>([]);

	// Share via Link active state
	isSharingLink = $state<boolean>(false);

	// Pending Incoming Transfer Modal State
	pendingIncoming = $state<IncomingTransferEventPayload | null>(null);

	// Pending Browser Download Requests (when share_link_auto_accept is true)
	pendingBrowserDownloads = $state<BrowserDownloadRequest[]>([]);

	// Active Tab ('radar' | 'transfers' | 'mesh')
	activeTab = $state<'radar' | 'transfers' | 'mesh'>('radar');

	// Multi-hop Mesh Overlay & Simulation State
	meshTopology = $state<MeshTopologyView | null>(null);
	meshLoading = $state<boolean>(false);
	selectedMeshNodeFp = $state<string | null>(null);
	tracerouteResult = $state<MeshTracerouteResponse | null>(null);
	speedtestResult = $state<SpeedtestResultDto | null>(null);
	isTracing = $state<boolean>(false);
	isBenchmarking = $state<boolean>(false);
	diagnosticError = $state<string | null>(null);
	severedLinks = $state<string[]>([]);

	// Active and Recent Transfers
	transfers = $state<TransferQueueItem[]>([]);

	// Selected peer for target sending dialog (single mode)
	selectedPeer = $state<DiscoveredPeer | null>(null);

	// UI Modals
	showSettings = $state<boolean>(false);
	showSendDialog = $state<boolean>(false);
	showTextModal = $state<boolean>(false);

	// Derived: Filtered Peers
	filteredPeers = $derived.by(() => {
		const ownFp = this.status?.fingerprint?.toUpperCase();
		let list = this.peers;
		if (ownFp) {
			list = list.filter((p) => p.fingerprint?.toUpperCase() !== ownFp);
		}
		// Strict deduplication by fingerprint, keeping newest
		const seen = new Set<string>();
		const deduped: DiscoveredPeer[] = [];
		for (const p of list) {
			const fp = p.fingerprint?.toUpperCase() || '';
			if (fp) {
				if (!seen.has(fp)) {
					seen.add(fp);
					deduped.push(p);
				}
			} else {
				deduped.push(p);
			}
		}
		if (!this.searchQuery.trim()) {
			return deduped;
		}
		const q = this.searchQuery.toLowerCase();
		return deduped.filter((p) => {
			return (
				p.alias.toLowerCase().includes(q) ||
				p.fingerprint.toLowerCase().includes(q) ||
				p.endpoints.some((e) => (typeof e === 'string' ? e.includes(q) : String(e).includes(q))) ||
				(p.deviceModel && p.deviceModel.toLowerCase().includes(q))
			);
		});
	});

	// Derived: Active Transfers Count
	activeTransferCount = $derived.by(() => {
		return this.transfers.filter((t) => t.status === 'active' || t.status === 'waiting').length;
	});

	// Derived: Total Prepared Size
	totalPreparedSize = $derived.by(() => {
		return this.preparedItems.reduce((acc, item) => acc + item.size, 0);
	});

	// Initialize app, load history, and setup event listeners
	async init() {
		// Setup event listeners first so no events are missed
		await this.setupListeners();

		// Load transfer history from backend / localStorage
		await this.loadPersistedHistory();

		try {
			this.status = await getNodeStatus();
			this.portConflict = !!this.status?.portConflict;
			try {
				this.serverPort = await startReceiverServer();
				this.portConflict = false;
			} catch (serverErr: any) {
				const errStr = String(serverErr).toLowerCase();
				if (errStr.includes('occupied') || errStr.includes('already in use') || errStr.includes('conflict')) {
					this.portConflict = true;
				}
				console.warn('Receiver server startup deferred:', serverErr);
			}
			await startDiscovery();
			this.isDiscovering = true;
			this.peers = await getDiscoveredPeers();
		} catch (e) {
			console.error('Failed to initialize app state:', e);
		}
	}

	private async loadPersistedHistory() {
		try {
			const persisted = await getTransferHistory();
			if (Array.isArray(persisted) && persisted.length > 0) {
				this.transfers = persisted;
			} else if (typeof localStorage !== 'undefined') {
				const raw = localStorage.getItem('lanflare_transfers');
				if (raw) {
					this.transfers = JSON.parse(raw);
				}
			}

			// Clean up any transfers that were left active/waiting when the app last terminated
			for (const t of this.transfers) {
				if (t.status === 'active' || t.status === 'waiting') {
					t.status = 'cancelled';
					t.error = 'Transfer interrupted by app shutdown';
				}
			}
		} catch (err) {
			console.warn('Failed to load transfer history:', err);
		}
	}

	persistHistory() {
		if (this.status && !this.status.saveToHistory) {
			return;
		}
		try {
			if (typeof localStorage !== 'undefined') {
				localStorage.setItem('lanflare_transfers', JSON.stringify(this.transfers));
			}
			saveTransferHistory(this.transfers).catch(() => {});
		} catch (e) {
			console.error('Failed to persist history:', e);
		}
	}

	deleteTransfer(id: string) {
		this.transfers = this.transfers.filter((t) => t.id !== id);
		this.persistHistory();
	}

	clearAllHistory() {
		// Keep only actively transferring or waiting items
		this.transfers = this.transfers.filter(
			(t) => t.status === 'active' || t.status === 'waiting'
		);
		this.persistHistory();
	}

	// Selection & Preparation Methods
	addFiles(filePaths: string[]) {
		for (const p of filePaths) {
			if (!this.preparedItems.some((item) => item.path === p)) {
				let rawName = p.split('/').pop()?.split('\\').pop() || p;
				try {
					rawName = decodeURIComponent(rawName);
				} catch {
					// keep rawName
				}
				if (p.startsWith('content://') && !rawName.includes('.')) {
					rawName = `Item_${rawName}`;
				}
				this.preparedItems.push({
					id: p,
					type: 'file',
					name: rawName,
					path: p,
					size: 0,
					summary: 'File'
				});
			}
		}
	}

	addFolder(folderPath: string) {
		if (!this.preparedItems.some((item) => item.path === folderPath)) {
			const name = folderPath.split('/').pop()?.split('\\').pop() || folderPath;
			this.preparedItems.push({
				id: folderPath,
				type: 'folder',
				name,
				path: folderPath,
				size: 0,
				summary: 'Folder'
			});
		}
	}

	async addTextMessage(text: string, title?: string) {
		try {
			const filePath = await createTempTextFile(text, title ? `${title}.txt` : undefined);
			const snippet = text.slice(0, 30).trim() + (text.length > 30 ? '...' : '');
			const name = title || (snippet ? `"${snippet}"` : 'Text Message');
			const size = new TextEncoder().encode(text).length;

			this.preparedItems.push({
				id: filePath,
				type: 'text',
				name,
				path: filePath,
				size,
				textContent: text,
				summary: 'Text Message'
			});
		} catch (e) {
			console.error('Failed to create text message file:', e);
		}
	}

	async pasteFromClipboard(): Promise<{ success: boolean; message?: string }> {
		try {
			if (isTauri()) {
				// 1. Try reading an image via official @tauri-apps/plugin-clipboard-manager
				try {
					const img = await tauriReadImage();
					if (img) {
						const rgba = await img.rgba();
						const size = await img.size();
						if (rgba && rgba.length > 0 && size.width > 0 && size.height > 0) {
							const filePath = await saveClipboardImage(rgba, size.width, size.height);
							const name = filePath.split('/').pop() || 'clipboard.png';
							this.preparedItems.push({
								id: filePath,
								type: 'file',
								name,
								path: filePath,
								size: rgba.length,
								summary: 'Pasted Image'
							});
							return { success: true, message: `Added image from clipboard: ${name}` };
						}
					}
				} catch {
					// No image or readImage threw, proceed to text
				}

				// 2. Try reading text via official @tauri-apps/plugin-clipboard-manager
				try {
					const text = await tauriReadText();
					if (text && text.trim()) {
						const trimmed = text.trim();
						// Check if text contains copied file paths / file:// URLs
						const lines = trimmed.split(/\r?\n/).map((l) => l.trim()).filter(Boolean);
						const filePaths: string[] = [];
						for (const line of lines) {
							if (line === 'copy' || line === 'cut' || line.startsWith('x-special/')) continue;
							let p = line;
							if (p.startsWith('file://')) {
								p = decodeURIComponent(p.slice(7));
								if (/^\/[a-zA-Z]:/.test(p)) {
									p = p.slice(1);
								}
							}
							if (p.startsWith('/') || /^[a-zA-Z]:[\\/]/.test(p)) {
								filePaths.push(p);
							}
						}

						if (filePaths.length > 0) {
							this.addFiles(filePaths);
							return { success: true, message: `Added ${filePaths.length} file(s) from clipboard` };
						}

						// Normal text message
						await this.addTextMessage(trimmed);
						return { success: true, message: 'Added text message from clipboard' };
					}
				} catch {
					// Proceed to backend fallback
				}

				// 3. Fallback to native Rust backend read_clipboard
				try {
					const content = await readClipboard();
					if (content.type === 'files') {
						if (content.data.length > 0) {
							this.addFiles(content.data);
							return { success: true, message: `Added ${content.data.length} file(s) from clipboard` };
						}
					} else if (content.type === 'image') {
						this.preparedItems.push({
							id: content.data.path,
							type: 'file',
							name: content.data.name,
							path: content.data.path,
							size: content.data.size,
							summary: 'Pasted Image'
						});
						return { success: true, message: `Added image from clipboard: ${content.data.name}` };
					} else if (content.type === 'text') {
						if (content.data && content.data.trim()) {
							await this.addTextMessage(content.data.trim());
							return { success: true, message: 'Added text message from clipboard' };
						}
					}
				} catch {
					// Fall through to error
				}

				return { success: false, message: 'Clipboard is empty or contains unsupported content' };
			} else {
				// Standalone browser dev mock fallback
				if (typeof navigator !== 'undefined' && navigator.clipboard) {
					const text = await navigator.clipboard.readText();
					if (text && text.trim()) {
						await this.addTextMessage(text.trim());
						return { success: true, message: 'Added text message from clipboard' };
					}
				}
				return { success: false, message: 'Clipboard is empty or does not contain text' };
			}
		} catch (e) {
			console.warn('Failed to read clipboard:', e);
			return { success: false, message: String(e) };
		}
	}

	removePreparedItem(index: number) {
		this.preparedItems = this.preparedItems.filter((_, i) => i !== index);
	}

	clearPreparedItems() {
		this.preparedItems = [];
	}

	toggleRecipient(fingerprint: string) {
		if (this.selectedRecipients.includes(fingerprint)) {
			this.selectedRecipients = this.selectedRecipients.filter((fp) => fp !== fingerprint);
		} else {
			this.selectedRecipients = [...this.selectedRecipients, fingerprint];
		}
	}

	selectAllRecipients() {
		this.selectedRecipients = this.peers.map((p) => p.fingerprint);
	}

	clearSelectedRecipients() {
		this.selectedRecipients = [];
	}

	async removePeer(fingerprint: string) {
		this.peers = this.peers.filter((p) => p.fingerprint !== fingerprint);
		this.selectedRecipients = this.selectedRecipients.filter((fp) => fp !== fingerprint);
		try {
			await removeDiscoveredPeer(fingerprint);
		} catch (e) {
			console.error('Failed to remove peer:', e);
		}
	}

	// Share via Link methods
	async startShareViaLink() {
		if (this.preparedItems.length === 0) return;
		try {
			await offerDownloadFiles(this.preparedItems.map((i) => i.path));
			this.isSharingLink = true;
		} catch (e) {
			console.error('Failed to start share via link:', e);
		}
	}

	async stopShareViaLink() {
		try {
			await clearDownloadFiles();
			this.isSharingLink = false;
		} catch (e) {
			console.error('Failed to stop share via link:', e);
		}
	}

	// Sending
	async sendPreparedToPeer(peer: DiscoveredPeer, pin?: string) {
		if (this.preparedItems.length === 0) return;
		const filePaths = this.preparedItems.map((i) => i.path);

		try {
			this.activeTab = 'transfers';
			await sendFiles({
				targetFingerprint: peer.fingerprint,
				targetEndpoint: peer.endpoints[0],
				files: filePaths,
				pin
			});

			// If in single recipient mode:
			// "Single recipient: Sends files to one recipient. Selection will be cleared after finished files transfer."
			if (this.sendMode === 'single') {
				this.clearPreparedItems();
			}
		} catch (e) {
			console.error(`Failed to send files to ${peer.alias}:`, e);
			throw e;
		}
	}

	async sendPreparedToMultiple(pin?: string) {
		if (this.preparedItems.length === 0 || this.selectedRecipients.length === 0) return;
		this.activeTab = 'transfers';
		const filePaths = this.preparedItems.map((i) => i.path);
		const targetPeers = this.peers.filter((p) => this.selectedRecipients.includes(p.fingerprint));

		for (const peer of targetPeers) {
			try {
				await sendFiles({
					targetFingerprint: peer.fingerprint,
					targetEndpoint: peer.endpoints[0],
					files: filePaths,
					pin
				});
			} catch (e) {
				console.error(`Failed to send files to ${peer.alias}:`, e);
			}
		}

		// "Multiple recipients : Sends files to multiple recipients. Selection will not be cleared after finished files transfer."
		// Selection is retained!
	}

	private async setupListeners() {
		// 1. Peer updates
		await tauriListen<void>('lanflare://peers-updated', async () => {
			try {
				this.peers = await getDiscoveredPeers();
			} catch (e) {
				console.error('Failed to reload peers:', e);
			}
		});

		// 1b. Port conflict and server status updates
		await tauriListen<boolean>('lanflare://port-conflict', (conflict) => {
			this.portConflict = conflict;
			if (!conflict && !this.serverPort && this.status) {
				this.serverPort = this.status.port;
			}
		});

		await tauriListen<number>('lanflare://server-started', async (port) => {
			this.serverPort = port;
			this.portConflict = false;
			try {
				await announcePresence();
				this.peers = await getDiscoveredPeers();
			} catch (e) {
				console.error('Failed to refresh peers after server started:', e);
			}
		});

		// 2. Incoming transfer request approval prompt
		await tauriListen<IncomingTransferEventPayload>(
			'lanflare://incoming-transfer-request',
			(payload) => {
				this.pendingIncoming = payload;
			}
		);

		// 2b. Browser download request approval prompt (when share_link_auto_accept is true)
		await tauriListen<BrowserDownloadRequest>(
			'lanflare://browser-download-request',
			(payload) => {
				if (!this.pendingBrowserDownloads.some((d) => d.downloadId === payload.downloadId)) {
					this.pendingBrowserDownloads.push(payload);
				}
			}
		);

		// 3. Incoming transfer auto-accepted
		await tauriListen<IncomingTransferEventPayload>('lanflare://incoming-transfer', (payload) => {
			this.transfers.unshift({
				id: payload.sessionId,
				direction: 'incoming',
				peerAlias: payload.senderAlias,
				peerEndpoint: payload.senderIp,
				files: payload.files.map((f) => ({
					id: f.id,
					name: f.fileName,
					size: f.size,
					bytesSent: 0,
					status: 'pending' as const,
					path: this.status?.downloadDir ? `${this.status.downloadDir}/${f.fileName}` : undefined
				})),
				totalBytes: payload.totalBytes,
				bytesTransferred: 0,
				speedBytesPerSec: 0,
				etaSeconds: null,
				status: 'active',
				startTime: Date.now()
			});
			this.activeTab = 'transfers';
			this.persistHistory();
		});

		// 4. File started
		await tauriListen<{ sessionId: string; fileId: string; fileName: string; totalBytes: number }>(
			'lanflare://file-started',
			(payload) => {
				const item = this.transfers.find((t) => t.id === payload.sessionId);
				if (item) {
					const f = item.files.find((x) => x.id === payload.fileId);
					if (f) {
						f.status = 'in_progress';
						if (!f.path && this.status?.downloadDir && payload.fileName) {
							f.path = `${this.status.downloadDir}/${payload.fileName}`;
						}
					}
					this.persistHistory();
				}
			}
		);

		// 5. File received
		await tauriListen<FileReceivedPayload>('lanflare://file-received', (payload) => {
			if (typeof window !== 'undefined') {
				const storageBridge = (window as any).AndroidStorage;
				if (storageBridge && payload.savedPath) {
					try {
						storageBridge.scanFile(payload.savedPath);
					} catch (_) {}
				}
			}
			const item = this.transfers.find((t) => t.id === payload.sessionId);
			if (item) {
				const f = item.files.find((x) => x.id === payload.fileId);
				if (f) {
					f.status = 'completed';
					f.bytesSent = payload.size;
					f.path = payload.savedPath;
				}
				item.bytesTransferred = item.files.reduce((acc, cur) => acc + (cur.bytesSent || 0), 0);
				if (item.bytesTransferred >= item.totalBytes) {
					item.status = 'completed';
				}
				this.persistHistory();
			}
		});


		// 6. Outgoing transfer initiated (waiting for peer to accept)
		await tauriListen<OutgoingTransferInitiatedPayload>(
			'lanflare://outgoing-transfer-initiated',
			(payload) => {
				const existing = this.transfers.find((t) => t.id === payload.sessionId);
				if (!existing) {
					const preparedMap = new Map(this.preparedItems.map((pi) => [pi.name, pi.path]));
					this.transfers.unshift({
						id: payload.sessionId,
						direction: 'outgoing',
						peerAlias: payload.targetAlias,
						peerEndpoint: payload.targetEndpoint,
						targetFingerprint: payload.targetFingerprint,
						files: payload.files.map((f) => ({
							id: f.id,
							name: f.fileName,
							size: f.size,
							bytesSent: 0,
							status: 'pending' as const,
							path: f.path || preparedMap.get(f.fileName) || undefined
						})),
						totalBytes: payload.totalBytes,
						bytesTransferred: 0,
						speedBytesPerSec: 0,
						etaSeconds: null,
						status: 'waiting',
						startTime: Date.now()
					});
					this.persistHistory();
				}
				this.activeTab = 'transfers';
			}
		);

		// 7. Outgoing transfer progress & status updates
		await tauriListen<TransferProgressPayload>('lanflare://transfer-progress', (payload) => {
			let item = this.transfers.find((t) => t.id === payload.sessionId);
			if (!item && payload.sessionId) {
				const waitingItem = this.transfers.find(
					(t) => t.direction === 'outgoing' && t.status === 'waiting'
				);
				if (waitingItem) {
					item = waitingItem;
					item.id = payload.sessionId;
				} else {
					item = {
						id: payload.sessionId,
						direction: 'outgoing',
						peerAlias: this.selectedPeer?.alias || 'Remote Peer',
						peerEndpoint: this.selectedPeer?.endpoints?.[0],
						files: [],
						totalBytes: payload.sessionTotalBytes || 0,
						bytesTransferred: payload.sessionBytesSent || 0,
						speedBytesPerSec: payload.bytesPerSecond || 0,
						etaSeconds: payload.etaSeconds ?? null,
						status:
							payload.eventType === 'started' || payload.eventType === 'file_progress'
								? 'active'
								: 'waiting',
						startTime: Date.now()
					};
					this.transfers.unshift(item);
				}
			}

			if (item) {
				if (payload.sessionBytesSent !== undefined) {
					item.bytesTransferred = payload.sessionBytesSent;
				}
				if (payload.sessionTotalBytes !== undefined && payload.sessionTotalBytes > 0) {
					item.totalBytes = payload.sessionTotalBytes;
				}
				if (payload.bytesPerSecond !== undefined) {
					item.speedBytesPerSec = payload.bytesPerSecond;
				}
				if (payload.etaSeconds !== undefined) {
					item.etaSeconds = payload.etaSeconds;
				}

				if (payload.eventType === 'started') {
					item.status = 'active';
				} else if (payload.eventType === 'file_progress' && payload.fileId) {
					item.status = 'active';
					let f = item.files.find((x) => x.id === payload.fileId);
					if (!f) {
						f = {
							id: payload.fileId,
							name: payload.fileName || 'file',
							size: payload.fileTotalBytes || 0,
							bytesSent: payload.fileBytesSent || 0,
							status: 'in_progress'
						};
						item.files.push(f);
					} else {
						f.bytesSent = payload.fileBytesSent || 0;
						f.status = 'in_progress';
					}
				} else if (payload.eventType === 'file_finished' && payload.fileId) {
					const f = item.files.find((x) => x.id === payload.fileId);
					if (f) {
						f.status = 'completed';
					}
				} else if (payload.eventType === 'session_completed') {
					item.status = 'completed';
					item.speedBytesPerSec = 0;
					item.etaSeconds = null;
					for (const f of item.files) {
						if (f.status !== 'failed') {
							f.status = 'completed';
							f.bytesSent = f.size;
						}
					}
					item.bytesTransferred = item.totalBytes;
				} else if (payload.eventType === 'session_cancelled') {
					item.status = 'cancelled';
					item.error = payload.error || 'Transfer cancelled by user';
					item.speedBytesPerSec = 0;
					item.etaSeconds = null;
				} else if (payload.eventType === 'session_failed') {
					if (payload.error && payload.error.toLowerCase().includes('declined')) {
						item.status = 'declined';
					} else {
						item.status = 'failed';
					}
					item.error = payload.error || 'Transfer failed';
					item.speedBytesPerSec = 0;
					item.etaSeconds = null;
				} else if (payload.eventType === 'file_failed') {
					if (payload.fileId) {
						const f = item.files.find((x) => x.id === payload.fileId);
						if (f) {
							f.status = 'failed';
							f.error = payload.error;
						}
					}
				}
				this.persistHistory();
			}
		});

		// 8. Mesh topology updates
		await tauriListen<MeshTopologyView>('lanflare://mesh-updated', (payload) => {
			this.meshTopology = payload;
			if (!this.selectedMeshNodeFp && payload.nodes.length > 0) {
				const nonLocal = payload.nodes.find((n) => n.fingerprint !== payload.localFingerprint);
				this.selectedMeshNodeFp = nonLocal ? nonLocal.fingerprint : payload.nodes[0].fingerprint;
			}
		});
	}

	// Actions
	async acceptIncoming() {
		if (this.pendingIncoming) {
			const payload = this.pendingIncoming;
			try {
				await respondIncomingTransfer(payload.sessionId, true);
				this.transfers.unshift({
					id: payload.sessionId,
					direction: 'incoming',
					peerAlias: payload.senderAlias,
					peerEndpoint: payload.senderIp,
					files: payload.files.map((f) => ({
						id: f.id,
						name: f.fileName,
						size: f.size,
						bytesSent: 0,
						status: 'pending',
						path: f.path || (this.status?.downloadDir ? `${this.status.downloadDir}/${f.fileName}` : undefined)
					})),
					totalBytes: payload.totalBytes,
					bytesTransferred: 0,
					speedBytesPerSec: 0,
					etaSeconds: null,
					status: 'active',
					startTime: Date.now()
				});
				this.activeTab = 'transfers';
				this.persistHistory();
			} catch (e) {
				console.error('Failed to accept transfer:', e);
			} finally {
				this.pendingIncoming = null;
			}
		}
	}

	async declineIncoming() {
		if (this.pendingIncoming) {
			try {
				await respondIncomingTransfer(this.pendingIncoming.sessionId, false);
			} catch (e) {
				console.error('Failed to decline transfer:', e);
			} finally {
				this.pendingIncoming = null;
			}
		}
	}

	async respondBrowserDownload(downloadId: string, accept: boolean) {
		try {
			await respondBrowserDownload(downloadId, accept);
		} catch (e) {
			console.error('Failed to respond to browser download request:', e);
		} finally {
			this.pendingBrowserDownloads = this.pendingBrowserDownloads.filter(
				(d) => d.downloadId !== downloadId
			);
		}
	}

	async cancelTransfer(sessionId: string) {
		try {
			await cancelOutgoingTransfer(sessionId);
			const item = this.transfers.find((t) => t.id === sessionId);
			if (item) {
				item.status = 'cancelled';
				this.persistHistory();
			}
		} catch (e) {
			console.error('Failed to cancel transfer:', e);
		}
	}

	async retryTransfer(transferId: string) {
		const item = this.transfers.find((t) => t.id === transferId);
		if (!item) return;

		// Get all files that have paths
		const filePaths = item.files
			.map((f) => f.path)
			.filter((p): p is string => typeof p === 'string' && p.length > 0);

		if (filePaths.length === 0) {
			console.error('Cannot retry transfer: file paths not found on disk');
			return;
		}

		item.status = 'waiting';
		item.error = undefined;
		item.bytesTransferred = 0;
		item.speedBytesPerSec = 0;
		item.etaSeconds = null;
		item.startTime = Date.now();
		for (const f of item.files) {
			f.status = 'pending';
			f.bytesSent = 0;
			f.error = undefined;
		}
		this.activeTab = 'transfers';
		this.persistHistory();

		try {
			const peer = this.peers.find(
				(p) =>
					(item.targetFingerprint && p.fingerprint === item.targetFingerprint) ||
					(item.peerEndpoint && p.endpoints.includes(item.peerEndpoint))
			);

			const newSessionId = await sendFiles({
				targetFingerprint: item.targetFingerprint || peer?.fingerprint || '',
				targetEndpoint: item.peerEndpoint || peer?.endpoints[0],
				files: filePaths
			});

			item.id = newSessionId;
			this.persistHistory();
		} catch (e) {
			item.status = 'failed';
			item.error = String(e);
			this.persistHistory();
		}
	}

	// Mesh Overlay & Simulation Methods
	async loadMeshTopology() {
		this.meshLoading = true;
		try {
			this.meshTopology = await getMeshTopology();
			if (this.meshTopology && !this.selectedMeshNodeFp && this.meshTopology.nodes.length > 0) {
				const nonLocal = this.meshTopology.nodes.find(
					(n) => n.fingerprint !== this.meshTopology?.localFingerprint
				);
				this.selectedMeshNodeFp = nonLocal
					? nonLocal.fingerprint
					: this.meshTopology.nodes[0].fingerprint;
			}
		} catch (e) {
			console.error('Failed to load mesh topology:', e);
		} finally {
			this.meshLoading = false;
		}
	}

	async startMeshSimulation(payload?: StartSimulationPayload) {
		this.meshLoading = true;
		try {
			this.meshTopology = await startMeshSimulation(payload);
			this.severedLinks = [];
			this.tracerouteResult = null;
			this.speedtestResult = null;
			this.diagnosticError = null;
			if (this.meshTopology && this.meshTopology.nodes.length > 0) {
				const nonLocal = this.meshTopology.nodes.find(
					(n) => n.fingerprint !== this.meshTopology?.localFingerprint
				);
				this.selectedMeshNodeFp = nonLocal
					? nonLocal.fingerprint
					: this.meshTopology.nodes[0].fingerprint;
			}
		} catch (e) {
			console.error('Failed to start mesh simulation:', e);
		} finally {
			this.meshLoading = false;
		}
	}

	async toggleMeshLink(fromFp: string, toFp: string, severed?: boolean) {
		const key1 = `${fromFp}:${toFp}`;
		const key2 = `${toFp}:${fromFp}`;
		const isCurrentlySevered =
			this.severedLinks.includes(key1) || this.severedLinks.includes(key2);
		const targetSevered = severed !== undefined ? severed : !isCurrentlySevered;

		try {
			const nowSevered = await toggleMeshLink({ fromFp, toFp, severed: targetSevered });
			if (nowSevered) {
				if (!this.severedLinks.includes(key1)) this.severedLinks.push(key1);
				if (!this.severedLinks.includes(key2)) this.severedLinks.push(key2);
			} else {
				this.severedLinks = this.severedLinks.filter((k) => k !== key1 && k !== key2);
			}
			await this.loadMeshTopology();
		} catch (e) {
			console.error('Failed to toggle mesh link:', e);
		}
	}

	async runMeshTraceroute(target: string) {
		this.isTracing = true;
		this.diagnosticError = null;
		this.tracerouteResult = null;
		try {
			this.tracerouteResult = await runMeshTraceroute({ target });
		} catch (e) {
			this.diagnosticError = `Traceroute error: ${e}`;
			console.error('Traceroute failed:', e);
		} finally {
			this.isTracing = false;
		}
	}

	async runMeshBenchmark(target: string) {
		this.isBenchmarking = true;
		this.diagnosticError = null;
		this.speedtestResult = null;
		try {
			this.speedtestResult = await runMeshBenchmark({ target });
		} catch (e) {
			this.diagnosticError = `Speedtest error: ${e}`;
			console.error('Speedtest failed:', e);
		} finally {
			this.isBenchmarking = false;
		}
	}
}

export const appState = new AppState();
