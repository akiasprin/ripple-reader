// Auto-extracted from the original monolithic app.js.
// Each module exports a slice of the global `app` object; main.js merges them.
export const theme = {
  _updateThemeColor(isDark, isDimmed) {
    let meta = document.querySelector('meta[name="theme-color"]');
    if (!meta) {
      meta = document.createElement('meta');
      meta.name = 'theme-color';
      document.head.appendChild(meta);
    }
    if (isDimmed) {
      // Blend card color at 35% opacity over bg: card * 0.35 + bg * 0.65
      meta.content = isDark ? '#1c1c1f' : '#f9f9fa';
    } else {
      meta.content = isDark ? '#18181b' : '#f5f6f8';
    }
  },

  initTheme() {
    const saved = localStorage.getItem('theme');
    const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
    const isDark = saved ? saved === 'dark' : prefersDark;
    if (isDark) document.documentElement.setAttribute('data-theme', 'dark');
    this.updateThemeIcon(isDark);
    const lightLink = document.getElementById('prism-light');
    const darkLink = document.getElementById('prism-dark');
    if (lightLink) lightLink.disabled = isDark;
    if (darkLink) darkLink.disabled = !isDark;
    const isDimmed0 = !!document.querySelector('.insight-dimmed');
    this._updateThemeColor(isDark, isDimmed0);
  },

  toggleTheme() {
    this.haptic('light');
    const isDark = document.documentElement.getAttribute('data-theme') === 'dark';
    const nextIsDark = !isDark;
    const apply = () => {
      if (isDark) {
        document.documentElement.removeAttribute('data-theme');
        localStorage.setItem('theme', 'light');
      } else {
        document.documentElement.setAttribute('data-theme', 'dark');
        localStorage.setItem('theme', 'dark');
      }
      this.updateThemeIcon(nextIsDark);
      const lightLink = document.getElementById('prism-light');
      const darkLink = document.getElementById('prism-dark');
      if (lightLink) lightLink.disabled = nextIsDark;
      if (darkLink) darkLink.disabled = !nextIsDark;
      const isDimmed = !!document.querySelector('.insight-dimmed');
      this._updateThemeColor(nextIsDark, isDimmed);
    };
    if (document.startViewTransition) {
      if (!nextIsDark) {
        document.documentElement.classList.add('theme-transition-reverse');
      }
      const transition = document.startViewTransition(apply);
      transition.finished.finally(() => {
        document.documentElement.classList.remove('theme-transition-reverse');
      });
    } else {
      apply();
    }
  },

  updateThemeIcon(isDark) {
    const btn = document.getElementById('themeToggle');
    if (!btn) return;
    if (isDark) {
      btn.innerHTML = '<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="5"/><path d="M12 1v2M12 21v2M4.22 4.22l1.42 1.42M18.36 18.36l1.42 1.42M1 12h2M21 12h2M4.22 19.78l1.42-1.42M18.36 5.64l1.42-1.42"/></svg>';
      btn.title = '切换为浅色';
    } else {
      btn.innerHTML = '<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"/></svg>';
      btn.title = '切换为深色';
    }
  },
};
