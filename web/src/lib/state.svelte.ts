import type {
	NodeStatusDto,
	DiscoveredPeer,
	IncomingTransferEventPayload,
	TransferProgressPayload,
	OutgoingTransferInitiatedPayload,
	TransferQueueItem,
	FileReceivedPayload
} from './types';
import {
	getNodeStatus,
	getDiscoveredPeers,
	startDiscovery,
	startReceiverServer,
	tauriListen,
	respondIncomingTransfer,
	cancelOutgoingTransfer
} from './api';

export class AppState {
	// Node Status
	status = $state<NodeStatusDto | null>(null);
	serverPort = $state<number | null>(null);
	isDiscovering = $state<boolean>(false);

	// Discovered Peers
	peers = $state<DiscoveredPeer[]>([]);
	searchQuery = $state<string>('');

	// Pending Incoming Transfer Modal State
	pendingIncoming = $state<IncomingTransferEventPayload | null>(null);

	// Active and Recent Transfers
	transfers = $state<TransferQueueItem[]>([]);

	// Selected peer for target sending dialog
	selectedPeer = $state<DiscoveredPeer | null>(null);

	// UI Modals
	showSettings = $state<boolean>(false);
	showSendDialog = $state<boolean>(false);

	// Derived: Filtered Peers
	filteredPeers = $derived.by(() => {
		if (!this.searchQuery.trim()) {
			return this.peers;
		}
		const q = this.searchQuery.toLowerCase();
		return this.peers.filter((p) => {
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
		return this.transfers.filter((t) => t.status === 'active').length;
	});

	// Initialize app and event listeners
	async init() {
		// Setup event listeners first so no events are missed
		await this.setupListeners();

		try {
			this.status = await getNodeStatus();
			this.serverPort = await startReceiverServer();
			await startDiscovery();
			this.isDiscovering = true;
			this.peers = await getDiscoveredPeers();
		} catch (e) {
			console.error('Failed to initialize app state:', e);
		}
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

		// 2. Incoming transfer request approval prompt
		await tauriListen<IncomingTransferEventPayload>(
			'lanflare://incoming-transfer-request',
			(payload) => {
				this.pendingIncoming = payload;
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
					status: 'pending'
				})),
				totalBytes: payload.totalBytes,
				bytesTransferred: 0,
				speedBytesPerSec: 0,
				etaSeconds: null,
				status: 'active',
				startTime: Date.now()
			});
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
					}
				}
			}
		);

		// 5. File received
		await tauriListen<FileReceivedPayload>('lanflare://file-received', (payload) => {
			const item = this.transfers.find((t) => t.id === payload.sessionId);
			if (item) {
				const f = item.files.find((x) => x.id === payload.fileId);
				if (f) {
					f.status = 'completed';
					f.bytesSent = payload.size;
				}
				item.bytesTransferred += payload.size;
				if (item.bytesTransferred >= item.totalBytes) {
					item.status = 'completed';
				}
			}
		});

		// 6. Outgoing transfer initiated (waiting for peer to accept)
		await tauriListen<OutgoingTransferInitiatedPayload>(
			'lanflare://outgoing-transfer-initiated',
			(payload) => {
				const existing = this.transfers.find((t) => t.id === payload.sessionId);
				if (!existing) {
					this.transfers.unshift({
						id: payload.sessionId,
						direction: 'outgoing',
						peerAlias: payload.targetAlias,
						peerEndpoint: payload.targetEndpoint,
						files: payload.files.map((f) => ({
							id: f.id,
							name: f.fileName,
							size: f.size,
							bytesSent: 0,
							status: 'pending'
						})),
						totalBytes: payload.totalBytes,
						bytesTransferred: 0,
						speedBytesPerSec: 0,
						etaSeconds: null,
						status: 'waiting',
						startTime: Date.now()
					});
				}
			}
		);

		// 7. Outgoing transfer progress & status updates
		await tauriListen<TransferProgressPayload>('lanflare://transfer-progress', (payload) => {
			let item = this.transfers.find((t) => t.id === payload.sessionId);
			if (!item && payload.sessionId) {
				// Register fallback if not found
				item = {
					id: payload.sessionId,
					direction: 'outgoing',
					peerAlias: this.selectedPeer?.alias || 'Remote Peer',
					files: [],
					totalBytes: payload.sessionTotalBytes || 0,
					bytesTransferred: payload.sessionBytesSent || 0,
					speedBytesPerSec: payload.bytesPerSecond || 0,
					etaSeconds: payload.etaSeconds ?? null,
					status: payload.eventType === 'started' || payload.eventType === 'file_progress' ? 'active' : 'waiting',
					startTime: Date.now()
				};
				this.transfers.unshift(item);
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
			}
		});
	}

	// Actions
	async acceptIncoming() {
		if (this.pendingIncoming) {
			const payload = this.pendingIncoming;
			try {
				await respondIncomingTransfer(payload.sessionId, true);
				// Add to active queue
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
						status: 'pending'
					})),
					totalBytes: payload.totalBytes,
					bytesTransferred: 0,
					speedBytesPerSec: 0,
					etaSeconds: null,
					status: 'active',
					startTime: Date.now()
				});
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

	async cancelTransfer(sessionId: string) {
		try {
			await cancelOutgoingTransfer(sessionId);
			const item = this.transfers.find((t) => t.id === sessionId);
			if (item) {
				item.status = 'cancelled';
			}
		} catch (e) {
			console.error('Failed to cancel transfer:', e);
		}
	}
}

export const appState = new AppState();
