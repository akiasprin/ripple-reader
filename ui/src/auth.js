// Auto-extracted from the original monolithic app.js.
// Each module exports a slice of the global `app` object; main.js merges them.
export const auth = {
  async initAuth() {
    this.authenticated = false;
    this.authToken = localStorage.getItem('authToken') || '';
    try {
      const headers = this.authToken ? { 'Authorization': 'Bearer ' + this.authToken } : {};
      const res = await fetch('/api/auth', { headers });
      const d = await res.json();
      this.authenticated = d.authenticated;
    } catch (e) {
      this.authenticated = false;
    }
    this.syncAuthCookie();
    this.updateAuthIcon();
  },

  syncAuthCookie() {
    // Marker cookie for nginx cache-bypass on logged-in requests.
    if (this.authenticated) {
      document.cookie = 'loggedin=1; path=/; max-age=2592000; SameSite=Lax';
    } else {
      document.cookie = 'loggedin=; path=/; max-age=0; SameSite=Lax';
    }
  },

  updateAuthIcon() {
    const btn = document.getElementById('authToggle');
    if (!btn) return;
    if (this.authenticated) {
      btn.innerHTML = '<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="11" width="18" height="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 9.9-1"/></svg>';
      btn.style.color = 'var(--mark-supporting)';
      btn.title = '点击登出';
    } else {
      btn.innerHTML = '<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="11" width="18" height="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/></svg>';
      btn.style.color = '';
      btn.title = '认证';
    }
    if (!this.insightPageId && !this.tagsPageOpen && !this.datesPageOpen) {
      this.renderList();
    }
    this.updateAdminLink();
  },

  updateAdminLink() {
    const link = document.getElementById('navAdmin');
    const divider = document.getElementById('navAdminDivider');
    const show = this.authenticated;
    if (link) link.style.display = show ? 'flex' : 'none';
    if (divider) divider.style.display = show ? '' : 'none';
  },

  openAuth() {
    const panel = document.getElementById('authPanel');
    panel.classList.add('open');
    this._showSpotlightOverlay();
    this.trapFocus(panel);
  },

  closeAuth() {
    const panel = document.getElementById('authPanel');
    const input = document.getElementById('authInput');
    if (input) input.blur();
    if (panel) {
      delete panel._returnFocus;
      panel.classList.remove('open');
    }
    this._hideSpotlightOverlay();
    this.untrapFocus(panel);
  },

  maybeCloseAuth() {
    setTimeout(() => {
      const input = document.getElementById('authInput');
      if (document.activeElement !== input && !input.value.trim()) {
        this.closeAuth();
      }
    }, 200);
  },

  toggleAuth() {
    if (this.authenticated) {
      this.logout();
      return;
    }
    const panel = document.getElementById('authPanel');
    const isOpen = panel.classList.contains('open');
    if (isOpen) {
      this.closeAuth();
    } else {
      panel.classList.add('open');
      this._showSpotlightOverlay();
      this.trapFocus(panel);
      setTimeout(() => document.getElementById('authInput').focus(), 50);
    }
  },

  logout() {
    this.authenticated = false;
    this.authToken = '';
    localStorage.removeItem('authToken');
    this.syncAuthCookie();
    this.updateAuthIcon();
  },

  async login() {
    const pw = document.getElementById('authInput').value;
    const msg = document.getElementById('authPanel').querySelector('.auth-msg');
    try {
      const res = await fetch('/api/auth', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ password: pw }),
      });
      if (res.ok) {
        const data = await res.json();
        this.authToken = data.token;
        localStorage.setItem('authToken', data.token);
        this.authenticated = true;
        this.syncAuthCookie();
        document.getElementById('authInput').value = '';
        this.closeAuth();
        this.updateAuthIcon();
      } else {
        this.showAuthMsg('密码错误');
      }
    } catch (e) {
      this.showAuthMsg('请求失败');
    }
  },

  showAuthMsg(text) {
    const panel = document.getElementById('authPanel');
    let msg = panel.querySelector('.auth-msg');
    if (!msg) { msg = document.createElement('span'); msg.className = 'auth-msg'; panel.appendChild(msg); }
    msg.textContent = text;
    msg.classList.remove('ok');
    setTimeout(() => { if (msg.textContent === text) msg.textContent = ''; }, 2000);
  },
};
