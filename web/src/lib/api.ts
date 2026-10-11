import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
	NodeStatusDto,
	UpdateConfigPayload,
	DiscoveredPeer,
	IncomingTransferEventPayload,
	TransferProgressPayload,
	SendFilesPayload
} from './types';

// Check if running inside Tauri webview
export const isTauri = (): boolean => {
	return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
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

// Discovery API
export async function getDiscoveredPeers(): Promise<DiscoveredPeer[]> {
	return tauriInvoke<DiscoveredPeer[]>('get_discovered_peers');
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

// Fallback mocks for Vite standalone dev/test in browser
function getMockResponse<T>(cmd: string, _args?: Record<string, unknown>): T {
	switch (cmd) {
		case 'get_node_status':
			return {
				alias: 'LanFlare Mock Node',
				port: 53317,
				downloadDir: '/home/user/Downloads',
				autoAccept: false,
				pinEnabled: false,
				fingerprint: '3FA85F647990AFDDF6872B2F139FF30B00000000000000000000000000000000',
				interfaces: [{ name: 'eth0', address: '192.168.1.100', isIpv6: false }]
			} as T;
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
		default:
			return undefined as T;
	}
}
