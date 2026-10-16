import { popIntentQueue } from 'tauri-plugin-mobile-sharetarget-api';
import { isAndroid, isTauri, tauriListen } from './api';
import { appState } from './state.svelte';

export interface ParsedShareIntent {
	type: 'file' | 'text' | 'unknown';
	streamUri?: string;
	text?: string;
	subject?: string;
	mimeType?: string;
}

/**
 * Parses an Android Intent URI string into its component parts.
 */
export function parseIntent(rawIntent: string): ParsedShareIntent {
	let streamUri: string | undefined;
	let text: string | undefined;
	let subject: string | undefined;
	let mimeType: string | undefined;

	// 1. Direct content URI before #Intent;
	const parts = rawIntent.split('#Intent;');
	const dataPart = parts[0];
	if (dataPart.startsWith('content://') || dataPart.startsWith('file://')) {
		streamUri = dataPart;
	}

	// 2. Parse semicolon-delimited fields
	const intentBody = parts[1] || parts[0];
	const fields = intentBody.split(';');

	for (const field of fields) {
		if (field.startsWith('type=')) {
			try {
				mimeType = decodeURIComponent(field.slice(5));
			} catch {
				mimeType = field.slice(5);
			}
		} else if (field.startsWith('S.android.intent.extra.STREAM=')) {
			const val = field.slice('S.android.intent.extra.STREAM='.length);
			try {
				streamUri = decodeURIComponent(val);
			} catch {
				streamUri = val;
			}
		} else if (field.startsWith('S.android.intent.extra.TEXT=')) {
			const val = field.slice('S.android.intent.extra.TEXT='.length);
			try {
				text = decodeURIComponent(val);
			} catch {
				text = val;
			}
		} else if (field.startsWith('S.android.intent.extra.SUBJECT=')) {
			const val = field.slice('S.android.intent.extra.SUBJECT='.length);
			try {
				subject = decodeURIComponent(val);
			} catch {
				subject = val;
			}
		}
	}

	if (streamUri) {
		return { type: 'file', streamUri, mimeType, text, subject };
	} else if (text) {
		return { type: 'text', text, subject, mimeType };
	}

	return { type: 'unknown' };
}

let isProcessing = false;

/**
 * Drains the mobile share target intent queue and updates appState.
 */
export async function processIntentQueue(): Promise<void> {
	if (!isTauri() || !isAndroid()) return;
	if (isProcessing) return;

	isProcessing = true;
	try {
		const fileUris: string[] = [];

		while (true) {
			let raw: string | null = null;
			try {
				raw = await popIntentQueue();
			} catch (e) {
				console.warn('[ShareTarget] popIntentQueue error:', e);
				break;
			}

			if (!raw) break;

			const parsed = parseIntent(raw);
			if (parsed.type === 'file' && parsed.streamUri) {
				fileUris.push(parsed.streamUri);
			} else if (parsed.type === 'text' && parsed.text) {
				await appState.addTextMessage(parsed.text, parsed.subject);
			}
		}

		if (fileUris.length > 0) {
			appState.addFiles(fileUris);
		}
	} finally {
		isProcessing = false;
	}
}

/**
 * Initializes the share target listener for Android.
 * Drains intents on startup, focus, and visibility changes.
 */
export function initShareTarget(): () => void {
	if (!isTauri() || !isAndroid()) {
		return () => {};
	}

	// 1. Initial drain
	processIntentQueue();

	// 2. Listen to Tauri window focus event
	let unlistenFocus: (() => void) | undefined;
	tauriListen('tauri://focus', () => {
		processIntentQueue();
	}).then((unlisten) => {
		unlistenFocus = unlisten;
	});

	// 3. Listen to browser visibility and focus events
	const handleVisibilityChange = () => {
		if (document.visibilityState === 'visible') {
			processIntentQueue();
		}
	};

	const handleWindowFocus = () => {
		processIntentQueue();
	};

	document.addEventListener('visibilitychange', handleVisibilityChange);
	window.addEventListener('focus', handleWindowFocus);

	return () => {
		if (unlistenFocus) unlistenFocus();
		document.removeEventListener('visibilitychange', handleVisibilityChange);
		window.removeEventListener('focus', handleWindowFocus);
	};
}
