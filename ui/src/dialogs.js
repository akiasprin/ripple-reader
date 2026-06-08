// Auto-extracted from the original monolithic app.js.
// Each module exports a slice of the global `app` object; main.js merges them.
export const dialogs = {
  openDialogWithOrigin(overlayId, clickX, clickY) {
    const overlay = document.getElementById(overlayId);
    const dialog = overlay.querySelector('.dialog');
    const oldAnim = this._dialogAnims.get(overlayId);
    if (oldAnim) oldAnim.cancel();
    // Clear any leftover WAAPI forwards-fill so getBoundingClientRect() measures the natural size
    dialog.getAnimations().forEach(a => { a.effect = null; });
    overlay.classList.add('open');

    // Lock body scroll when dialog opens to prevent scroll-chaining on mobile.
    // On iOS Safari, position:fixed body combined with the soft keyboard causes
    // coordinate-mapping bugs that make dialog buttons untappable, so we only
    // use overflow:hidden. The fixed overlay already blocks all touch events.
    const isMobile = !this.isDesktopViewport();
    if (isMobile) {
      const scrollY = window.scrollY || document.documentElement.scrollTop || 0;
      document.documentElement.style.overflow = 'hidden';
      dialog._bodyScrollLocked = true;
      dialog._savedScrollY = scrollY;
    }

    if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
      this._dialogAnims.delete(overlayId);
      return;
    }
    const rect = dialog.getBoundingClientRect();
    const originX = clickX !== undefined ? clickX - rect.left : rect.width / 2;
    const originY = clickY !== undefined ? clickY - rect.top : rect.height / 2;
    dialog.style.transformOrigin = `${originX}px ${originY}px`;
    dialog.style.willChange = 'transform, opacity';
    const anim = dialog.animate([
      { transform: 'scale(0)', opacity: 0 },
      { transform: 'scale(1)', opacity: 1 }
    ], {
      duration: 300,
      easing: 'cubic-bezier(0.16, 1, 0.3, 1)',
      fill: 'forwards'
    });
    this._dialogAnims.set(overlayId, anim);
  },

  closeDialogWithOrigin(overlayId, onFinish) {
    const overlay = document.getElementById(overlayId);
    const dialog = overlay.querySelector('.dialog');

    const anim = this._dialogAnims.get(overlayId);
    const finish = () => {
      // Unlock body scroll AFTER animation ends — unlocking before causes
      // iOS Safari to recompute the viewport and reset <meta name="theme-color">.
      if (dialog._bodyScrollLocked) {
        const scrollY = dialog._savedScrollY || 0;
        document.documentElement.style.overflow = '';
        window.scrollTo(0, scrollY);
        delete dialog._bodyScrollLocked;
        delete dialog._savedScrollY;
      }
      overlay.classList.remove('open');
      dialog.style.transformOrigin = '';
      dialog.style.willChange = '';
      dialog.style.height = '';
      this._dialogAnims.delete(overlayId);
      if (onFinish) onFinish();
    };
    if (anim && !window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
      anim.cancel();
      const reverseAnim = dialog.animate([
        { transform: 'scale(1)', opacity: 1 },
        { transform: 'scale(0)', opacity: 0 }
      ], {
        duration: 200,
        easing: 'ease-in',
        fill: 'forwards'
      });
      this._dialogAnims.set(overlayId, reverseAnim);
      reverseAnim.onfinish = finish;
    } else {
      finish();
    }
  },

  trapFocus(container, returnFocusTo, autoFocus = true) {
    // Skip if already trapped — prevents duplicate keydown listeners
    if (container._trapHandler) return;
    const focusable = Array.from(container.querySelectorAll('button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])'));
    // Skip close buttons for auto-focus target — they don't need the ring
    const firstInput = focusable.find(el => !el.classList.contains('close-btn') && !el.classList.contains('dialog-close-btn'));
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    container._returnFocus = returnFocusTo || document.activeElement;
    container._trapHandler = (e) => {
      if (e.key !== 'Tab') return;
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault(); last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault(); first.focus();
      }
    };
    container.addEventListener('keydown', container._trapHandler);
    if (autoFocus && firstInput) setTimeout(() => firstInput.focus(), 0);
  },

  untrapFocus(container) {
    if (container._trapHandler) {
      container.removeEventListener('keydown', container._trapHandler);
      delete container._trapHandler;
    }
    const returnTo = container._returnFocus;
    if (returnTo && returnTo.focus) { returnTo.focus(); delete container._returnFocus; }
  },

  _showSpotlightOverlay() {
    const overlay = document.getElementById('spotlightOverlay');
    if (overlay) overlay.classList.add('show');
  },

  _hideSpotlightOverlay() {
    const overlay = document.getElementById('spotlightOverlay');
    if (overlay) overlay.classList.remove('show');
  },

  closeSpotlight() {
    this.closeSearch();
    this.closeAuth();
  },

  scrollToTop() {
    window.scrollTo({ top: 0, behavior: 'smooth' });
  },

  scrollToBottom() {
    window.scrollTo({ top: document.body.scrollHeight, behavior: 'smooth' });
  },

  _animateOverlayOpen(overlay, clickX, clickY) {
    overlay.classList.add('open');
    if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) return;
    overlay.style.transformOrigin = `${clickX !== undefined ? clickX : overlay.offsetWidth / 2}px ${clickY !== undefined ? clickY : overlay.offsetHeight / 2}px`;
    overlay.style.willChange = 'transform, opacity';
    overlay.animate([
      { transform: 'scale(0)', opacity: 0 },
      { transform: 'scale(1)', opacity: 1 }
    ], {
      duration: 300,
      easing: 'cubic-bezier(0.16, 1, 0.3, 1)',
      fill: 'forwards'
    });
  },

  restoreDialog({ title, body, width }, clickX, clickY) {
    const overlay = document.createElement('div');
    overlay.id = 'modalOverlay';
    overlay.className = 'dialog-overlay';
    const dialog = document.createElement('div');
    dialog.className = 'dialog';
    if (width) dialog.style.maxWidth = width;
    dialog.innerHTML = `
      <div class="dialog-header">
        <h3>${title}</h3>
        <button class="dialog-close-btn" onclick="app.hideModal()">&times;</button>
      </div>
      <div class="dialog-body">${body}</div>
    `;
    overlay.appendChild(dialog);
    overlay.addEventListener('click', e => { if (e.target === overlay) this.hideModal(); });
    document.body.appendChild(overlay);
    overlay.classList.add('open');
    if (!window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
      const oldAnim = this._dialogAnims.get('modalOverlay');
      if (oldAnim) oldAnim.cancel();
      const rect = dialog.getBoundingClientRect();
      const originX = clickX !== undefined ? clickX - rect.left : rect.width / 2;
      const originY = clickY !== undefined ? clickY - rect.top : rect.height / 2;
      dialog.style.transformOrigin = `${originX}px ${originY}px`;
      dialog.style.willChange = 'transform, opacity';
      const anim = dialog.animate([
        { transform: 'scale(0)', opacity: 0 },
        { transform: 'scale(1)', opacity: 1 }
      ], {
        duration: 300,
        easing: 'cubic-bezier(0.16, 1, 0.3, 1)',
        fill: 'forwards'
      });
      this._dialogAnims.set('modalOverlay', anim);
    }
    this.trapFocus(overlay);
    // Avoid focusing the close button by default; move focus to the first button in the body
    const firstBodyBtn = overlay.querySelector('.dialog-body button, .dialog-body a[href]');
    if (firstBodyBtn) setTimeout(() => firstBodyBtn.focus(), 0);
  },

  hideModal() {
    const overlay = document.getElementById('modalOverlay');
    if (!overlay) return;
    this.untrapFocus(overlay);
    const dialog = overlay.querySelector('.dialog');
    const anim = this._dialogAnims.get('modalOverlay');
    if (anim && !window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
      anim.cancel();
      const reverseAnim = dialog.animate([
        { transform: 'scale(1)', opacity: 1 },
        { transform: 'scale(0)', opacity: 0 }
      ], {
        duration: 200,
        easing: 'ease-in',
        fill: 'forwards'
      });
      this._dialogAnims.set('modalOverlay', reverseAnim);
      reverseAnim.onfinish = () => {
        if (overlay.parentNode) overlay.remove();
        this._dialogAnims.delete('modalOverlay');
      };
    } else {
      overlay.remove();
      this._dialogAnims.delete('modalOverlay');
    }
  },
};
