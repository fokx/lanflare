import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { openPath, revealItemInDir } from '@tauri-apps/plugin-opener';
import type {
	NodeStatusDto,
	UpdateConfigPayload,
	DiscoveredPeer,
	IncomingTransferEventPayload,
	TransferProgressPayload,
	SendFilesPayload,
	ClipboardContentDto,
	DirListingDto,
	QuickDirDto,
	StartDufsShareDto,
	DufsShareStatusDto,
	MeshTopologyView,
	StartSimulationPayload,
	ToggleMeshLinkPayload,
	RunTraceroutePayload,
	MeshTracerouteResponse,
	RunBenchmarkPayload,
	SpeedtestResultDto,
	ChatConversationDto,
	ChatMessageViewDto,
	SendChatMessagePayload,
	RespondTrustRequestPayload,
	CreateChatGroupPayload,
	GroupMembershipRosterDto,
	SyncChatGroupPayload,
	SyncStatsDto,
	UniversalClipboardPayloadDto,
	ClipboardStateDto,
	SearchHitDto,
	SearchFederatedFilesPayload,
	ScratchpadSyncDto,
	ScratchpadEditDto
} from './types';

// Open a file using system default application
export async function openFile(filePath: string): Promise<void> {
	if (!filePath) return;
	if (isAndroid()) {
		const androidStorage = typeof window !== 'undefined' ? (window as any).AndroidStorage : null;
		if (androidStorage?.openFile) {
			try {
				androidStorage.openFile(filePath);
				return;
			} catch (e) {
				console.error('Failed to open file via AndroidStorage:', e);
			}
		}
	}
	if (isTauri()) {
		try {
			await tauriInvoke('open_path_in_system', { path: filePath });
			return;
		} catch (e) {
			console.warn('open_path_in_system command failed, falling back to plugin-opener:', e);
		}
		try {
			await openPath(filePath);
		} catch (e) {
			console.error('Failed to open file via opener:', e);
		}
	} else {
		console.log('[Web Mock] openFile:', filePath);
	}
}

// Open / reveal the folder containing a file or the folder itself in file manager, selecting the file
export async function openFolder(folderOrFilePath: string): Promise<void> {
	if (!folderOrFilePath) return;
	if (isAndroid()) {
		const androidStorage = typeof window !== 'undefined' ? (window as any).AndroidStorage : null;
		if (androidStorage?.openFolder) {
			try {
				androidStorage.openFolder(folderOrFilePath);
				return;
			} catch (e) {
				console.error('Failed to open folder via AndroidStorage:', e);
			}
		}
	}
	if (isTauri()) {
		try {
			await tauriInvoke('reveal_in_folder', { path: folderOrFilePath });
			return;
		} catch (e) {
			console.warn('reveal_in_folder command failed, falling back to plugin-opener:', e);
		}
		try {
			await revealItemInDir(folderOrFilePath);
		} catch (_) {
			try {
				await openPath(folderOrFilePath);
			} catch (e2) {
				console.error('Failed to reveal/open folder:', e2);
			}
		}
	} else {
		console.log('[Web Mock] openFolder:', folderOrFilePath);
	}
}

// Check if running inside Tauri webview
export const isTauri = (): boolean => {
	return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
};

// Check if running on Android platform
export const isAndroid = (): boolean => {
	return typeof navigator !== 'undefined' && /android/i.test(navigator.userAgent);
};

// Safe wrapper around invoke for browser mock fallback during web development
export async function tauriInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
	if (!isTauri()) {
		console.warn(`[Tauri Mock] invoke("${cmd}", ${JSON.stringify(args)})`);
		return getMockResponse<T>(cmd, args);
	}
	return await invoke<T>(cmd, args);
}

// Safe wrapper around listen
export async function tauriListen<T>(
	event: string,
	handler: (payload: T) => void
): Promise<UnlistenFn> {
	if (!isTauri()) {
		console.log(`[Tauri Mock] Subscribed to "${event}"`);
		return () => {};
	}
	return await listen<T>(event, (e) => handler(e.payload));
}

// Node & Config API
export async function getNodeStatus(): Promise<NodeStatusDto> {
	return tauriInvoke<NodeStatusDto>('get_node_status');
}

export async function updateConfig(payload: UpdateConfigPayload): Promise<NodeStatusDto> {
	return tauriInvoke<NodeStatusDto>('update_config', { payload });
}

export async function regenerateIdentity(): Promise<NodeStatusDto> {
	return tauriInvoke<NodeStatusDto>('regenerate_identity');
}

export async function generateRandomAlias(): Promise<string> {
	return tauriInvoke<string>('generate_random_alias');
}

// Discovery API
export async function getDiscoveredPeers(): Promise<DiscoveredPeer[]> {
	return tauriInvoke<DiscoveredPeer[]>('get_discovered_peers');
}

export async function removeDiscoveredPeer(fingerprint: string): Promise<boolean> {
	return tauriInvoke<boolean>('remove_discovered_peer', { fingerprint });
}

export async function startDiscovery(): Promise<void> {
	return tauriInvoke<void>('start_discovery');
}

export async function announcePresence(): Promise<void> {
	return tauriInvoke<void>('announce_presence');
}

export async function stopDiscovery(): Promise<void> {
	return tauriInvoke<void>('stop_discovery');
}

// Receiver API
export async function startReceiverServer(): Promise<number> {
	return tauriInvoke<number>('start_receiver_server');
}

export async function respondIncomingTransfer(sessionId: string, accept: boolean): Promise<void> {
	return tauriInvoke<void>('respond_incoming_transfer', { sessionId, accept });
}

export async function stopReceiverServer(): Promise<void> {
	return tauriInvoke<void>('stop_receiver_server');
}

// Transfer API
export async function sendFiles(payload: SendFilesPayload): Promise<string> {
	return tauriInvoke<string>('send_files', { payload });
}

export async function cancelOutgoingTransfer(sessionId: string): Promise<void> {
	return tauriInvoke<void>('cancel_outgoing_transfer', { sessionId });
}

// Text & Clipboard Transfer API
export async function createTempTextFile(content: string, fileName?: string): Promise<string> {
	return tauriInvoke<string>('create_temp_text_file', { content, fileName });
}

export async function readClipboard(): Promise<ClipboardContentDto> {
	return tauriInvoke<ClipboardContentDto>('read_clipboard');
}

export async function saveClipboardImage(rgba: Uint8Array, width: number, height: number): Promise<string> {
	return tauriInvoke<string>('save_clipboard_image', { rgba, width, height });
}

// Share via link API
export async function offerDownloadFiles(files: string[]): Promise<void> {
	return tauriInvoke<void>('offer_download_files', { files });
}

export async function clearDownloadFiles(): Promise<void> {
	return tauriInvoke<void>('clear_download_files');
}

export async function getDownloadFiles(): Promise<any[]> {
	return tauriInvoke<any[]>('get_download_files');
}

// History Persistence API
export async function getTransferHistory(): Promise<any[]> {
	return tauriInvoke<any[]>('get_transfer_history');
}

export async function saveTransferHistory(items: any[]): Promise<void> {
	return tauriInvoke<void>('save_transfer_history', { items });
}

export async function clearTransferHistory(): Promise<void> {
	return tauriInvoke<void>('clear_transfer_history');
}

export async function regenerateShareLinkPin(): Promise<string> {
	return tauriInvoke<string>('regenerate_share_link_pin');
}

export async function respondBrowserDownload(downloadId: string, accept: boolean): Promise<void> {
	return tauriInvoke<void>('respond_browser_download', { downloadId, accept });
}

// Directory Navigation & Dufs Folder Sharing API
export async function getCommonDirectories(): Promise<QuickDirDto[]> {
	return tauriInvoke<QuickDirDto[]>('get_common_directories');
}

export async function listDirectory(path?: string): Promise<DirListingDto> {
	return tauriInvoke<DirListingDto>('list_directory', { path });
}

export async function startDufsShare(options: StartDufsShareDto): Promise<DufsShareStatusDto> {
	return tauriInvoke<DufsShareStatusDto>('start_dufs_share', { options });
}

export async function stopDufsShare(): Promise<void> {
	return tauriInvoke<void>('stop_dufs_share');
}

export async function getDufsShareStatus(): Promise<DufsShareStatusDto | null> {
	return tauriInvoke<DufsShareStatusDto | null>('get_dufs_share_status');
}

// Multi-Hop Mesh Overlay & Simulation API
export async function getMeshTopology(): Promise<MeshTopologyView> {
	return tauriInvoke<MeshTopologyView>('get_mesh_topology');
}

export async function startMeshSimulation(payload?: StartSimulationPayload): Promise<MeshTopologyView> {
	return tauriInvoke<MeshTopologyView>('start_mesh_simulation', { payload });
}

export async function toggleMeshLink(payload: ToggleMeshLinkPayload): Promise<boolean> {
	return tauriInvoke<boolean>('toggle_mesh_link', { payload });
}

export async function runMeshTraceroute(payload: RunTraceroutePayload): Promise<MeshTracerouteResponse> {
	return tauriInvoke<MeshTracerouteResponse>('run_mesh_traceroute', { payload });
}

export async function runMeshBenchmark(payload: RunBenchmarkPayload): Promise<SpeedtestResultDto> {
	return tauriInvoke<SpeedtestResultDto>('run_mesh_benchmark', { payload });
}

// Encrypted 1:1 and Causal Group Chat API
export async function sendChatMessage(payload: SendChatMessagePayload): Promise<any> {
	return tauriInvoke<any>('send_chat_message', { payload });
}

export async function respondTrustRequest(payload: RespondTrustRequestPayload): Promise<void> {
	return tauriInvoke<void>('respond_trust_request', { payload });
}

export async function listChatConversations(): Promise<ChatConversationDto[]> {
	return tauriInvoke<ChatConversationDto[]>('list_chat_conversations');
}

export async function getChatHistory(payload: { conversationId: string; limit?: number }): Promise<ChatMessageViewDto[]> {
	return tauriInvoke<ChatMessageViewDto[]>('get_chat_history', { payload });
}

export async function createChatGroup(payload: CreateChatGroupPayload): Promise<GroupMembershipRosterDto> {
	return tauriInvoke<GroupMembershipRosterDto>('create_chat_group', { payload });
}

export async function syncChatGroup(payload: SyncChatGroupPayload): Promise<SyncStatsDto> {
	return tauriInvoke<SyncStatsDto>('sync_chat_group', { payload });
}

// LAN Toolbox: Clipboard Ring, Federated Search & Collab Scratchpad API
export async function getClipboardState(): Promise<ClipboardStateDto> {
	return tauriInvoke<ClipboardStateDto>('get_clipboard_state');
}

export async function broadcastClipboard(payload: { payload: UniversalClipboardPayloadDto }): Promise<boolean> {
	return tauriInvoke<boolean>('broadcast_clipboard', { payload });
}

export async function searchFederatedFiles(payload: SearchFederatedFilesPayload): Promise<SearchHitDto[]> {
	return tauriInvoke<SearchHitDto[]>('search_federated_files', { payload });
}

export async function getCollabDoc(payload: { docId: string; title?: string }): Promise<ScratchpadSyncDto> {
	return tauriInvoke<ScratchpadSyncDto>('get_collab_doc', { payload });
}

export async function applyCollabEdit(payload: { edit: ScratchpadEditDto }): Promise<boolean> {
	return tauriInvoke<boolean>('apply_collab_edit', { payload });
}

// Fallback mocks for Vite standalone dev/test in browser
function getMockResponse<T>(cmd: string, _args?: Record<string, unknown>): T {
	switch (cmd) {
		case 'get_mesh_topology':
		case 'start_mesh_simulation':
			return {
				localFingerprint: 'LOCAL_NODE_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
				relayEnabled: true,
				nodes: [
					{
						fingerprint: 'LOCAL_NODE_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
						alias: 'This Device (Local)',
						relayEnabled: true,
						seqNo: 1,
						lastSeenMs: Date.now()
					},
					{
						fingerprint: 'RELAY_1_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
						alias: 'Relay-1 (Living Room)',
						relayEnabled: true,
						seqNo: 2,
						lastSeenMs: Date.now()
					},
					{
						fingerprint: 'BOB_NODE_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
						alias: 'Bob (Subnet B)',
						relayEnabled: false,
						seqNo: 1,
						lastSeenMs: Date.now()
					}
				],
				links: [
					{
						srcFp: 'LOCAL_NODE_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
						dstFp: 'RELAY_1_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
						rttMs: 12,
						subnetTag: 'SubnetA',
						bandwidthKbps: 100000,
						lastSeenMs: Date.now()
					},
					{
						srcFp: 'RELAY_1_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
						dstFp: 'BOB_NODE_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
						rttMs: 18,
						subnetTag: 'SubnetB',
						bandwidthKbps: 80000,
						lastSeenMs: Date.now()
					}
				],
				routeAdvertisements: [
					{
						originFp: 'BOB_NODE_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
						seqNo: 1,
						routePath: [
							'BOB_NODE_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
							'RELAY_1_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789'
						],
						ttl: 4,
						metricRttMs: 30
					}
				]
			} as T;
		case 'toggle_mesh_link':
			return true as T;
		case 'run_mesh_traceroute':
			return {
				traceId: 'mock-trace-1234',
				originFp: 'LOCAL_NODE_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
				targetFp: 'BOB_NODE_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
				success: true,
				totalHops: 2,
				totalRttMs: 30,
				hops: [
					{
						nodeFp: 'RELAY_1_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
						alias: 'Relay-1',
						rttMs: 12,
						ingressTimestampMs: Date.now() - 20
					},
					{
						nodeFp: 'BOB_NODE_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
						alias: 'Bob',
						rttMs: 18,
						ingressTimestampMs: Date.now()
					}
				]
			} as T;
		case 'run_mesh_benchmark':
			return {
				testId: 'mock-bench-1234',
				originFp: 'LOCAL_NODE_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
				targetFp: 'BOB_NODE_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
				durationMs: 42,
				totalBytes: 1310720,
				throughputMbps: 249.6,
				chunksReceived: 20
			} as T;
		case 'get_node_status':
			return {
				alias: 'LanFlare Mock Node',
				port: 53317,
				downloadDir: '/home/user/Downloads',
				autoAccept: false,
				pinEnabled: false,
				fingerprint: '3FA85F647990AFDDF6872B2F139FF30B00000000000000000000000000000000',
				saveToHistory: true,
				multicastAddress: '224.0.0.167',
				shareLinkAutoAccept: false,
				shareLinkRequirePin: true,
				shareLinkPin: 'k7m9xp',
				interfaces: [{ name: 'eth0', address: '192.168.1.100', isIpv6: false }],
				portConflict: false
			} as T;
		case 'regenerate_share_link_pin':
			return 'k7m9xp' as T;
		case 'regenerate_identity':
			return {
				alias: 'LanFlare Mock Node',
				port: 53317,
				downloadDir: '/home/user/Downloads',
				autoAccept: false,
				pinEnabled: false,
				fingerprint: 'NEW85F647990AFDDF6872B2F139FF30B00000000000000000000000000000000',
				saveToHistory: true,
				multicastAddress: '224.0.0.167',
				shareLinkAutoAccept: false,
				shareLinkRequirePin: true,
				shareLinkPin: 'k7m9xp',
				interfaces: [{ name: 'eth0', address: '192.168.1.100', isIpv6: false }],
				portConflict: false
			} as T;
		case 'generate_random_alias':
			return 'Golden Mango' as T;
		case 'respond_browser_download':
			return undefined as T;
		case 'get_discovered_peers':
			return [
				{
					fingerprint: 'A1B2C3D4E5F678901234567890ABCDEF1234567890ABCDEF1234567890ABCDEF',
					alias: 'MacBook Pro (Living Room)',
					version: '2.0',
					deviceModel: 'MacBookPro18,1',
					deviceType: 'Desktop',
					endpoints: ['192.168.1.150:53317'],
					protocol: 'https',
					port: 53317,
					download: false,
					sources: 3,
					lastSeen: Date.now()
				},
				{
					fingerprint: 'F0E1D2C3B4A596877869504132231405F0E1D2C3B4A596877869504132231405',
					alias: 'Pixel 9 Phone',
					version: '2.0',
					deviceModel: 'Google Pixel 9',
					deviceType: 'Mobile',
					endpoints: ['192.168.1.188:53317'],
					protocol: 'https',
					port: 53317,
					download: false,
					sources: 1,
					lastSeen: Date.now()
				}
			] as T;
		case 'start_receiver_server':
			return 53317 as T;
		case 'send_files':
			return 'mock-session-uuid-1234' as T;
		case 'create_temp_text_file':
			return '/tmp/lanflare_messages/mock_message.txt' as T;
		case 'save_clipboard_image':
			return '/tmp/lanflare_clipboard/mock_clipboard.png' as T;
		case 'read_clipboard':
			return { type: 'text', data: 'Pasted clipboard mock content' } as T;
		case 'get_transfer_history':
		case 'get_download_files':
			return [] as T;
		case 'get_common_directories':
			return [
				{ label: 'Downloads', path: '/storage/emulated/0/Download', exists: true },
				{ label: 'Documents', path: '/storage/emulated/0/Documents', exists: true },
				{ label: 'DCIM', path: '/storage/emulated/0/DCIM', exists: true },
				{ label: 'Pictures', path: '/storage/emulated/0/Pictures', exists: true }
			] as T;
		case 'list_directory':
			return {
				currentPath: '/storage/emulated/0/Download',
				parentPath: '/storage/emulated/0',
				isAndroid: true,
				entries: [
					{ name: 'Photos', path: '/storage/emulated/0/Download/Photos', isDir: true, size: 0, modified: Date.now() / 1000 },
					{ name: 'document.pdf', path: '/storage/emulated/0/Download/document.pdf', isDir: false, size: 2048576, modified: Date.now() / 1000 }
				],
				commonPaths: [
					{ label: 'Downloads', path: '/storage/emulated/0/Download', exists: true },
					{ label: 'Documents', path: '/storage/emulated/0/Documents', exists: true }
				]
			} as T;
		case 'start_dufs_share':
			return {
				isActive: true,
				folderPath: '/storage/emulated/0/Download',
				folderName: 'Download',
				port: 53317,
				urls: ['https://192.168.1.100:53317/'],
				allowUpload: true,
				allowDelete: false,
				authPin: undefined,
				fileCount: 42,
				totalSize: 104857600
			} as T;
		case 'get_dufs_share_status':
			return null as T;
		case 'stop_dufs_share':
			return undefined as T;
		case 'list_chat_conversations':
			return [
				{
					id: 'CONV_MOCK_BOB_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
					alias: 'Bob (Workstation)',
					targetFp: 'BOB_NODE_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
					isGroup: false,
					lastMessage: 'Hey! Are you online on the local mesh?',
					lastMessageTimeMs: Date.now() - 60000,
					unreadCount: 1,
					trustState: 'TrustedContact'
				},
				{
					id: 'GROUP_MOCK_CORE_DEVS_ID',
					alias: 'LanFlare Core Mesh Group',
					targetFp: 'GROUP_MOCK_CORE_DEVS_ID',
					isGroup: true,
					lastMessage: '[Group message #4f2a91b0]',
					lastMessageTimeMs: Date.now() - 120000,
					unreadCount: 0,
					trustState: 'TrustedGroup'
				},
				{
					id: 'CONV_MOCK_STRANGER_FINGERPRINT',
					alias: 'Untrusted Guest Node',
					targetFp: 'STRANGER_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
					isGroup: false,
					lastMessage: 'Hello, I would like to connect over LAN.',
					lastMessageTimeMs: Date.now() - 300000,
					unreadCount: 1,
					trustState: 'Stranger'
				}
			] as T;
		case 'get_chat_history':
			return [
				{
					id: 'msg-1',
					senderFp: 'BOB_NODE_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
					recipientFp: 'LOCAL_NODE_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
					isSelf: false,
					content: 'Hey! Are you online on the local mesh?',
					timestampMs: Date.now() - 180000,
					deliveryState: 'Read',
					mediaType: 'text'
				},
				{
					id: 'msg-2',
					senderFp: 'LOCAL_NODE_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
					recipientFp: 'BOB_NODE_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
					isSelf: true,
					content: 'Yes! Connected via 2-hop overlay relay.',
					timestampMs: Date.now() - 120000,
					deliveryState: 'Delivered',
					mediaType: 'text'
				},
				{
					id: 'msg-3',
					senderFp: 'LOCAL_NODE_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
					recipientFp: 'BOB_NODE_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
					isSelf: true,
					content: '[Voice: 4s]',
					timestampMs: Date.now() - 60000,
					deliveryState: 'Delivered',
					mediaType: 'voice',
					voiceDurationSec: 4
				}
			] as T;
		case 'send_chat_message':
			return {
				id: 'msg-mock-new',
				timestampMs: Date.now(),
				content: { text: 'Mock message sent' }
			} as T;
		case 'respond_trust_request':
			return undefined as T;
		case 'create_chat_group':
			return {
				groupId: 'group-mock-new',
				groupName: 'New LAN Group',
				epoch: 1,
				members: [],
				adminFp: 'LOCAL_NODE_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
				createdAtMs: Date.now()
			} as T;
		case 'sync_chat_group':
			return {
				nodesPushed: 0,
				nodesPulled: 2,
				keysExchanged: 1
			} as T;
		case 'get_clipboard_state':
			return {
				ringId: 'RING_MOCK_UUID_1234',
				ringName: "Alice's Personal Devices",
				currentHash: 'E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855',
				payload: {
					type: 'text',
					text: 'https://github.com/lanflare/lanflare'
				},
				devices: [
					{
						fingerprint: 'LOCAL_NODE_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
						alias: 'This Laptop (Local)',
						deviceType: 'Desktop',
						joinedAtMs: Date.now() - 86400000
					},
					{
						fingerprint: 'PHONE_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
						alias: 'Personal Phone',
						deviceType: 'Mobile',
						joinedAtMs: Date.now() - 43200000
					}
				]
			} as T;
		case 'broadcast_clipboard':
			return true as T;
		case 'search_federated_files':
			return [
				{
					fileName: 'architecture_diagram.png',
					relativePath: '/shared/docs/architecture_diagram.png',
					sizeBytes: 1548200,
					modifiedAtMs: Date.now() - 3600000,
					mimeType: 'image/png',
					requiresPin: false,
					downloadUrl: 'http://192.168.1.150:53317/dufs/shared/docs/architecture_diagram.png',
					peerFp: 'BOB_NODE_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
					peerAlias: 'Bob (Workstation)',
					relevanceScore: 98
				},
				{
					fileName: 'lanflare_release_v2.tar.gz',
					relativePath: '/builds/lanflare_release_v2.tar.gz',
					sizeBytes: 24580210,
					modifiedAtMs: Date.now() - 7200000,
					mimeType: 'application/gzip',
					requiresPin: true,
					downloadUrl: 'http://192.168.1.188:53317/dufs/builds/lanflare_release_v2.tar.gz',
					peerFp: 'RELAY_1_MOCK_FINGERPRINT_64CHARS_ABCDEF0123456789ABCDEF0123456789',
					peerAlias: 'Relay-1 (Living Room)',
					relevanceScore: 85
				}
			] as T;
		case 'get_collab_doc':
			return {
				docId: 'scratchpad-main',
				title: 'Shared Project Notes.md',
				atoms: [
					{
						id: { lamport: 1, authorFp: 'AUTHOR_A', counter: 1 },
						value: '#',
						deleted: false,
						lamport: 1
					},
					{
						id: { lamport: 2, authorFp: 'AUTHOR_A', counter: 2 },
						afterId: { lamport: 1, authorFp: 'AUTHOR_A', counter: 1 },
						value: ' ',
						deleted: false,
						lamport: 2
					},
					{
						id: { lamport: 3, authorFp: 'AUTHOR_A', counter: 3 },
						afterId: { lamport: 2, authorFp: 'AUTHOR_A', counter: 2 },
						value: 'L',
						deleted: false,
						lamport: 3
					},
					{
						id: { lamport: 4, authorFp: 'AUTHOR_A', counter: 4 },
						afterId: { lamport: 3, authorFp: 'AUTHOR_A', counter: 3 },
						value: 'a',
						deleted: false,
						lamport: 4
					},
					{
						id: { lamport: 5, authorFp: 'AUTHOR_A', counter: 5 },
						afterId: { lamport: 4, authorFp: 'AUTHOR_A', counter: 4 },
						value: 'n',
						deleted: false,
						lamport: 5
					},
					{
						id: { lamport: 6, authorFp: 'AUTHOR_A', counter: 6 },
						afterId: { lamport: 5, authorFp: 'AUTHOR_A', counter: 5 },
						value: 'F',
						deleted: false,
						lamport: 6
					},
					{
						id: { lamport: 7, authorFp: 'AUTHOR_A', counter: 7 },
						afterId: { lamport: 6, authorFp: 'AUTHOR_A', counter: 6 },
						value: 'l',
						deleted: false,
						lamport: 7
					},
					{
						id: { lamport: 8, authorFp: 'AUTHOR_A', counter: 8 },
						afterId: { lamport: 7, authorFp: 'AUTHOR_A', counter: 7 },
						value: 'a',
						deleted: false,
						lamport: 8
					},
					{
						id: { lamport: 9, authorFp: 'AUTHOR_A', counter: 9 },
						afterId: { lamport: 8, authorFp: 'AUTHOR_A', counter: 8 },
						value: 'r',
						deleted: false,
						lamport: 9
					},
					{
						id: { lamport: 10, authorFp: 'AUTHOR_A', counter: 10 },
						afterId: { lamport: 9, authorFp: 'AUTHOR_A', counter: 9 },
						value: 'e',
						deleted: false,
						lamport: 10
					}
				],
				lamport: 10,
				lastModifiedMs: Date.now()
			} as T;
		case 'apply_collab_edit':
			return true as T;
		default:
			return undefined as T;
	}
}
