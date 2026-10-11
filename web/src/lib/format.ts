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
