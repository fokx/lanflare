export interface NetworkInterfaceDto {
	name: string;
	address: string;
	isIpv6: boolean;
}

export interface NodeStatusDto {
	alias: string;
	port: number;
	downloadDir: string;
	autoAccept: boolean;
	pinEnabled: boolean;
	fingerprint: string;
	saveToHistory: boolean;
	multicastAddress: string;
	shareLinkAutoAccept: boolean;
	shareLinkRequirePin: boolean;
	shareLinkPin: string;
	interfaces: NetworkInterfaceDto[];
	portConflict: boolean;
}

export interface UpdateConfigPayload {
	alias?: string;
	port?: number;
	downloadDir?: string;
	autoAccept?: boolean;
	pin?: string | null;
	saveToHistory?: boolean;
	multicastAddress?: string;
	shareLinkAutoAccept?: boolean;
	shareLinkRequirePin?: boolean;
	shareLinkPin?: string;
}

export interface BrowserDownloadRequest {
	downloadId: string;
	remoteIp: string;
	fileName: string;
	fileSize: number;
}

export type SendMode = 'single' | 'multiple' | 'link' | 'dufs';

export interface QuickDirDto {
	label: string;
	path: string;
	exists: boolean;
}

export interface DirEntryDto {
	name: string;
	path: string;
	isDir: boolean;
	size: number;
	modified: number;
}

export interface DirListingDto {
	currentPath: string;
	parentPath: string | null;
	entries: DirEntryDto[];
	isAndroid: boolean;
	commonPaths: QuickDirDto[];
}

export interface StartDufsShareDto {
	folderPath: string;
	allowUpload: boolean;
	allowDelete: boolean;
	requirePin: boolean;
	pin?: string;
}

export interface DufsShareStatusDto {
	isActive: boolean;
	folderPath: string;
	folderName: string;
	port: number;
	urls: string[];
	allowUpload: boolean;
	allowDelete: boolean;
	authPin?: string;
	fileCount: number;
	totalSize: number;
}

export type ClipboardContentDto =
	| { type: 'files'; data: string[] }
	| { type: 'image'; data: { path: string; name: string; size: number } }
	| { type: 'text'; data: string };

export interface PreparedSendItem {
	id: string;
	type: 'file' | 'folder' | 'text';
	name: string;
	path: string;
	size: number;
	summary?: string;
	textContent?: string;
}

export type DeviceType = 'mobile' | 'desktop' | 'web' | 'headless' | 'server' | 'Mobile' | 'Desktop' | 'Web' | 'Headless' | 'Server';
export type ProtocolType = 'http' | 'https';

export interface DiscoveredPeer {
	fingerprint: string;
	alias: string;
	version: string;
	deviceModel?: string;
	deviceType?: DeviceType;
	endpoints: string[];
	protocol: ProtocolType;
	port: number;
	download: boolean;
	sources: number;
	lastSeen: number;
}

export interface IncomingFileInfo {
	id: string;
	fileName: string;
	size: number;
	fileType?: string;
	path?: string;
}

export interface IncomingTransferEventPayload {
	sessionId: string;
	senderAlias: string;
	senderIp: string;
	files: IncomingFileInfo[];
	totalBytes: number;
}

export interface SendFilesPayload {
	targetFingerprint: string;
	targetEndpoint?: string;
	files: string[];
	pin?: string;
}

export interface TransferProgressPayload {
	sessionId: string;
	eventType:
		| 'started'
		| 'file_progress'
		| 'file_finished'
		| 'file_failed'
		| 'session_completed'
		| 'session_cancelled'
		| 'session_failed';
	fileId?: string;
	fileName?: string;
	fileBytesSent?: number;
	fileTotalBytes?: number;
	sessionBytesSent?: number;
	sessionTotalBytes?: number;
	bytesPerSecond?: number;
	etaSeconds?: number;
	error?: string;
}

export interface OutgoingTransferInitiatedPayload {
	sessionId: string;
	targetAlias: string;
	targetEndpoint?: string;
	targetFingerprint?: string;
	files: IncomingFileInfo[];
	totalBytes: number;
}

export interface FileReceivedPayload {
	sessionId: string;
	fileId: string;
	fileName: string;
	size: number;
	savedPath: string;
	sha256: string;
}

export interface TransferQueueItem {
	id: string;
	direction: 'outgoing' | 'incoming';
	peerAlias: string;
	peerEndpoint?: string;
	targetFingerprint?: string;
	files: Array<{
		id: string;
		name: string;
		size: number;
		bytesSent: number;
		status: 'pending' | 'in_progress' | 'completed' | 'failed';
		path?: string;
		error?: string;
	}>;
	totalBytes: number;
	bytesTransferred: number;
	speedBytesPerSec: number;
	etaSeconds: number | null;
	status: 'waiting' | 'active' | 'completed' | 'cancelled' | 'failed' | 'declined';
	error?: string;
	startTime: number;
}

// ============================================================================
// Multi-Hop Mesh Overlay & Simulation Types
// ============================================================================

export interface MeshNode {
	fingerprint: string;
	alias: string;
	certPem?: string;
	capabilities?: string[];
	relayEnabled: boolean;
	seqNo: number;
	lastSeenMs: number;
}

export interface MeshLink {
	srcFp: string;
	dstFp: string;
	rttMs: number;
	subnetTag?: string;
	bandwidthKbps?: number;
	lastSeenMs: number;
}

export interface MeshRouteAdvertisement {
	originFp: string;
	seqNo: number;
	routePath: string[];
	ttl: number;
	metricRttMs: number;
	capabilities?: string[];
	signature?: string;
}

export interface MeshTopologyView {
	localFingerprint: string;
	relayEnabled: boolean;
	nodes: MeshNode[];
	links: MeshLink[];
	routeAdvertisements: MeshRouteAdvertisement[];
}

export interface StartSimulationPayload {
	nodeCount?: number;
	subnets?: string[];
	latencyMs?: number;
}

export interface ToggleMeshLinkPayload {
	fromFp: string;
	toFp: string;
	severed?: boolean;
}

export interface RunTraceroutePayload {
	target: string;
	ttl?: number;
}

export interface TracerouteHop {
	nodeFp: string;
	alias: string;
	rttMs: number;
	ingressTimestampMs: number;
}

export interface MeshTracerouteResponse {
	traceId: string;
	originFp: string;
	targetFp: string;
	success: boolean;
	totalHops: number;
	totalRttMs: number;
	hops: TracerouteHop[];
	error?: string;
}

export interface RunBenchmarkPayload {
	target: string;
	chunks?: number;
	chunkSize?: number;
}

export interface SpeedtestResultDto {
	testId: string;
	originFp: string;
	targetFp: string;
	durationMs: number;
	totalBytes: number;
	throughputMbps: number;
	chunksReceived: number;
}

// ============================================================================
// P2P & Causal Group Chat Types
// ============================================================================

export type TrustState = 'Stranger' | 'PendingApproval' | 'TrustedContact' | 'Blocked' | 'TrustedGroup';

export interface ChatConversationDto {
	id: string;
	alias: string;
	targetFp: string;
	isGroup: boolean;
	lastMessage?: string;
	lastMessageTimeMs: number;
	unreadCount: number;
	trustState: string;
}

export interface ChatMessageViewDto {
	id: string;
	senderFp: string;
	recipientFp: string;
	isSelf: boolean;
	content: string;
	timestampMs: number;
	deliveryState: string; // 'Queued' | 'Sending' | 'Relayed' | 'Delivered' | 'Read' | 'Failed'
	mediaType?: 'text' | 'image' | 'file' | 'voice' | 'video';
	fileName?: string;
	fileSize?: number;
	bytesTransferred?: number;
	voiceDurationSec?: number;
	mediaUrl?: string;
}

export interface SendChatMessagePayload {
	target: string;
	message: string;
	intro?: boolean;
}

export interface RespondTrustRequestPayload {
	peerFingerprint: string;
	approve: boolean;
}

export interface CreateChatGroupPayload {
	groupName: string;
	memberFingerprints: string[];
}

export interface GroupMemberDto {
	fingerprint: string;
	alias: string;
	role: 'Admin' | 'Member';
	certPem: string;
	joinedAtMs: number;
}

export interface GroupMembershipRosterDto {
	groupId: string;
	groupName: string;
	epoch: number;
	members: GroupMemberDto[];
	adminFp: string;
	createdAtMs: number;
}

export interface SyncChatGroupPayload {
	groupId: string;
	peerFingerprint: string;
}

export interface SyncStatsDto {
	nodesPushed: number;
	nodesPulled: number;
	keysExchanged: number;
}

// ============================================================================
// Multi-Functional LAN Toolbox Types
// ============================================================================

export type UniversalClipboardPayloadDto =
	| { type: 'text'; text: string }
	| { type: 'image'; mimeType: string; dataBase64: string }
	| { type: 'files'; files: string[] };

export interface RingDeviceDto {
	fingerprint: string;
	alias: string;
	deviceType?: DeviceType;
	joinedAtMs: number;
}

export interface ClipboardStateDto {
	ringId: string;
	ringName: string;
	currentHash?: string;
	payload?: UniversalClipboardPayloadDto;
	devices: RingDeviceDto[];
}

export interface SearchHitDto {
	fileName: string;
	relativePath: string;
	sizeBytes: number;
	modifiedAtMs: number;
	mimeType: string;
	sha256?: string;
	requiresPin: boolean;
	downloadUrl: string;
	peerFp: string;
	peerAlias: string;
	relevanceScore: number;
}

export interface SearchFederatedFilesPayload {
	query: string;
	pin?: string;
	timeoutSecs?: number;
}

export interface AtomIdDto {
	lamport: number;
	authorFp: string;
	counter: number;
}

export interface ScratchpadAtomDto {
	id: AtomIdDto;
	afterId?: AtomIdDto;
	value: string;
	deleted: boolean;
	lamport: number;
}

export interface ScratchpadSyncDto {
	docId: string;
	title: string;
	atoms: ScratchpadAtomDto[];
	lamport: number;
	lastModifiedMs: number;
}

export type ScratchpadOpDto =
	| { type: 'insert'; atomId: AtomIdDto; afterId?: AtomIdDto; value: string }
	| { type: 'delete'; atomId: AtomIdDto };

export interface ScratchpadEditDto {
	docId: string;
	editId: string;
	authorFp: string;
	lamport: number;
	op: ScratchpadOpDto;
	timestampMs: number;
}


