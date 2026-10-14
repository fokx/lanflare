export type ThemeMode = 'system' | 'light' | 'dark';

export class ThemeManager {
	current = $state<ThemeMode>('system');
	isDark = $state<boolean>(false);

	private mediaQuery: MediaQueryList | null = null;
	private mediaListener: ((e: MediaQueryListEvent) => void) | null = null;

	init() {
		if (typeof window === 'undefined') return;

		// Load saved preference if any
		const saved = localStorage.getItem('lanflare_theme') as ThemeMode | null;
		if (saved && (saved === 'system' || saved === 'light' || saved === 'dark')) {
			this.current = saved;
		} else {
			this.current = 'system';
		}

		if (window.matchMedia) {
			this.mediaQuery = window.matchMedia('(prefers-color-scheme: dark)');
			this.mediaListener = (e: MediaQueryListEvent) => {
				if (this.current === 'system') {
					this.applyTheme(e.matches);
				}
			};
			this.mediaQuery.addEventListener('change', this.mediaListener);
		}

		this.evaluate();
	}

	setMode(mode: ThemeMode) {
		this.current = mode;
		if (typeof window !== 'undefined') {
			localStorage.setItem('lanflare_theme', mode);
		}
		this.evaluate();
	}

	private evaluate() {
		if (this.current === 'dark') {
			this.applyTheme(true);
		} else if (this.current === 'light') {
			this.applyTheme(false);
		} else {
			// 'system': detect system dark settings; if detection fails or unavailable, default to light theme
			let systemPrefersDark = false;
			try {
				if (typeof window !== 'undefined' && window.matchMedia) {
					const mq = window.matchMedia('(prefers-color-scheme: dark)');
					// If the media query media string evaluates and matches
					systemPrefersDark = mq.matches;
				}
			} catch {
				systemPrefersDark = false;
			}
			this.applyTheme(systemPrefersDark);
		}
	}

	private applyTheme(dark: boolean) {
		this.isDark = dark;
		if (typeof document !== 'undefined') {
			const root = document.documentElement;
			if (dark) {
				root.classList.add('dark');
				root.style.colorScheme = 'dark';
			} else {
				root.classList.remove('dark');
				root.style.colorScheme = 'light';
			}
		}
	}

	destroy() {
		if (this.mediaQuery && this.mediaListener) {
			this.mediaQuery.removeEventListener('change', this.mediaListener);
		}
	}
}

export const themeManager = new ThemeManager();
