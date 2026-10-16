// Format bytes into human-readable strings (KB, MB, GB)
export function formatBytes(bytes: number, decimals = 1): string {
	if (bytes === 0) return '0 B';
	const k = 1024;
	const dm = decimals < 0 ? 0 : decimals;
	const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
	const i = Math.floor(Math.log(bytes) / Math.log(k));
	return `${parseFloat((bytes / Math.pow(k, i)).toFixed(dm))} ${sizes[i]}`;
}

// Format transfer speed
export function formatSpeed(bytesPerSec: number): string {
	if (!bytesPerSec || bytesPerSec <= 0) return '0 B/s';
	return `${formatBytes(bytesPerSec, 1)}/s`;
}

// Format duration / ETA
export function formatEta(seconds: number | null | undefined): string {
	if (seconds === null || seconds === undefined || !isFinite(seconds) || seconds < 0) {
		return '--';
	}
	if (seconds < 60) {
		return `${Math.round(seconds)}s`;
	}
	const mins = Math.floor(seconds / 60);
	const secs = Math.round(seconds % 60);
	if (mins < 60) {
		return `${mins}m ${secs}s`;
	}
	const hrs = Math.floor(mins / 60);
	const remMins = mins % 60;
	return `${hrs}h ${remMins}m`;
}

// Format SHA-256 fingerprint for display (e.g. 3FA8...5A21)
export function shortenFingerprint(fp: string): string {
	if (!fp) return '';
	if (fp.length <= 12) return fp;
	return `${fp.slice(0, 6)}...${fp.slice(-6)}`;
}

/**
 * Extracts password/PIN from a raw input string or full URL.
 * Handles cases like:
 * - https://:b7asiojoij@@:e7z3@10.40.1.133:53317 -> "b7asiojoij@@:e7z3"
 * - http://user:secretPass@192.168.1.5:53317 -> "secretPass"
 * - http://192.168.1.5:53317/?pin=secretPass -> "secretPass"
 * - plain password -> plain password
 */
export function extractPasswordFromUrl(input: string): string {
	const trimmed = input.trim();
	if (!trimmed) return '';

	let authorityAndBeyond: string | null = null;
	const schemeIdx = trimmed.indexOf('://');
	if (schemeIdx !== -1) {
		authorityAndBeyond = trimmed.slice(schemeIdx + 3);
	} else if (trimmed.startsWith('//')) {
		authorityAndBeyond = trimmed.slice(2);
	} else {
		const lastAt = trimmed.lastIndexOf('@');
		if (lastAt !== -1) {
			const afterAt = trimmed.slice(lastAt + 1);
			if (!afterAt.startsWith(':') && (afterAt.includes('.') || afterAt.includes(':'))) {
				authorityAndBeyond = trimmed;
			}
		}
	}

	if (authorityAndBeyond !== null) {
		// Separate authority from path / query / fragment
		const endAuthIdx = authorityAndBeyond.search(/[/?#]/);
		const authority = endAuthIdx !== -1 ? authorityAndBeyond.slice(0, endAuthIdx) : authorityAndBeyond;
		const queryPart = endAuthIdx !== -1 ? authorityAndBeyond.slice(endAuthIdx) : '';

		const lastAt = authority.lastIndexOf('@');
		if (lastAt !== -1) {
			const userinfo = authority.slice(0, lastAt);
			let pass = userinfo;
			if (userinfo.startsWith(':')) {
				pass = userinfo.slice(1);
			} else {
				const colonIdx = userinfo.indexOf(':');
				if (colonIdx !== -1) {
					pass = userinfo.slice(colonIdx + 1);
				}
			}
			try {
				return decodeURIComponent(pass);
			} catch {
				return pass;
			}
		}

		// Check query string for pin=
		const qIdx = queryPart.indexOf('?');
		if (qIdx !== -1) {
			const qStr = queryPart.slice(qIdx + 1);
			for (const pair of qStr.split('&')) {
				if (pair.startsWith('pin=')) {
					const val = pair.slice(4).split('#')[0];
					try {
						return decodeURIComponent(val);
					} catch {
						return val;
					}
				}
			}
		}
	}

	return trimmed;
}

