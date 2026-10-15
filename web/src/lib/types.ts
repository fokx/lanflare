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
	interfaces: NetworkInterfaceDto[];
}

export interface UpdateConfigPayload {
	alias?: string;
	port?: number;
	downloadDir?: string;
	autoAccept?: boolean;
	pin?: string | null;
	saveToHistory?: boolean;
	multicastAddress?: string;
}

export type SendMode = 'single' | 'multiple' | 'link';

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
	files: Array<{
		id: string;
		name: string;
		size: number;
		bytesSent: number;
		status: 'pending' | 'in_progress' | 'completed' | 'failed';
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
