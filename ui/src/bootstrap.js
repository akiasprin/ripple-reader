// Auto-extracted from the original monolithic app.js.
// Each module exports a slice of the global `app` object; main.js merges them.
export const bootstrap = {
  async init() {
    // Load insight providers
    try {
      const res = await fetch('/api/insight-providers');
      if (res.ok) {
        const data = await res.json();
        this.insightProviders = data.providers || [];
        if (this.insightProviders.length > 0) {
          this.selectedInsightProvider = this.insightProviders[0];
          const select = document.getElementById('insightProviderDropdown');
          if (select) {
            select.innerHTML = this.insightProviders.map(p =>
              `<option value="${this.escape(p)}">${this.escape(p)}</option>`
            ).join('');
            select.addEventListener('change', () => {
              const requestedProvider = select.value;
              this.selectedInsightProvider = requestedProvider;
              // If insight dialog is open, reload prompt for the new provider
              const overlay = document.getElementById('insightOverlay');
              if (overlay && overlay.classList.contains('open')) {
                const ta = document.getElementById('insightDialogPrompt');
                const hint = document.getElementById('insightPromptHint');
                if (ta) {
                  ta.readOnly = true;
                  ta.placeholder = '加载中...';
                  this.loadPrompts(requestedProvider).then(prompts => {
                    // Guard against stale responses from rapid provider switching
                    if (select.value !== requestedProvider) return;
                    ta.readOnly = false;
                    ta.placeholder = '';
                    if (prompts) {
                      ta.value = prompts.insight || '';
                      if (hint) this.updatePromptHint(hint, prompts);
                    }
                  });
                }
              }
            });
          }
          const reviewSelect = document.getElementById('reviewProviderDropdown');
          if (reviewSelect) {
            reviewSelect.innerHTML = this.insightProviders.map(p =>
              `<option value="${this.escape(p)}">${this.escape(p)}</option>`
            ).join('');
            reviewSelect.addEventListener('change', () => {
              this.selectedInsightProvider = reviewSelect.value;
            });
          }
        }
      }
    } catch (e) {
      console.error('[init] Failed to load insight providers:', e);
    }
    document.getElementById('searchInput').addEventListener('keydown', e => {
      if (e.key === 'Enter') this.search();
    });
    document.addEventListener('keydown', async e => {
      if (e.key === 'Escape') {
        const el = id => document.getElementById(id);
        if (el('backupDiffOverlay')?.classList.contains('open')) { this.closeBackupDiff(); return; }
        if (el('insightCommentsPanel')?.classList.contains('open')) { this.hideCommentsPanel(); return; }
        if (el('orphanDrawer')?.classList.contains('open')) { this.hideOrphanPanel(); return; }
        if (el('insightOverlay')?.classList.contains('open')) { this.cancelInsightDialog(); return; }
        if (el('reviewOverlay')?.classList.contains('open')) { this.cancelReview(); return; }
        if (el('searchPanel')?.classList.contains('open')) { this.closeSearch(); return; }
        if (el('authPanel')?.classList.contains('open')) { this.closeAuth(); return; }
        return;
      }
      // Cmd+B / Ctrl+B to toggle theme
      if ((e.metaKey || e.ctrlKey) && e.key === 'b') {
        e.preventDefault();
        this.toggleTheme();
        return;
      }
      // Cmd+E / Ctrl+E to toggle first AI reply bubble
      if ((e.metaKey || e.ctrlKey) && e.key === 'e') {
        e.preventDefault();
        if (document.getElementById('replyBubble')) {
          this.hideReplyBubble();
        } else {
          const firstAi = this.comments?.find(c => c.is_ai && c.ai_reply && c.ai_status !== 'pending');
          if (firstAi) this.showReplyModal(firstAi.id, null);
        }
        return;
      }
      const tag = (e.target?.tagName || '').toLowerCase();
      if (tag === 'input' || tag === 'textarea' || tag === 'select') return;
      if (this.insightPageId) {
        if (this.insightPageEditing) return;
        if (e.key === 'ArrowLeft' && this.insightNeighbors.prev) {
          e.preventDefault();
          this.showInsightPage(this.insightNeighbors.prev.id, this.insightNeighbors.prev.source_type);
        } else if (e.key === 'ArrowRight' && this.insightNeighbors.next) {
          e.preventDefault();
          this.showInsightPage(this.insightNeighbors.next.id, this.insightNeighbors.next.source_type);
        }
      } else if (!this.tagsPageOpen && !this.datesPageOpen) {
        const cards = document.querySelectorAll('.paper-card');
        if (!cards.length) return;
        if (e.key === 'ArrowRight') {
          e.preventDefault();
          if (this.data) {
            const totalPages = Math.ceil(this.data.total / this.data.page_size) || 1;
            if (this.data.page < totalPages) await this.goPage(this.data.page + 1);
          }
        } else if (e.key === 'ArrowLeft') {
          e.preventDefault();
          if (this.data && this.data.page > 1) await this.goPage(this.data.page - 1);
        }
      }
    });
    window.addEventListener('popstate', async () => {
      this.syncFromURL();
      if (this.tagsPageOpen) {
        this.showTagsPage();
        return;
      }
      if (this.datesPageOpen) {
        this.showDatesPage();
        return;
      }
      if (this.insightPageId) {
        await this.showInsightPage(this.insightPageId, this.insightPageSource, true);
      } else {
        await this.load();
        this.exitInsightView();
        // Restore list scroll position after popstate back-navigation
        if (!this.isDesktopViewport() && this._listScrollY != null) {
          const savedY = this._listScrollY;
          this._listScrollY = null;
          requestAnimationFrame(() => {
            requestAnimationFrame(() => window.scrollTo(0, savedY));
          });
        }
      }
    });
    document.addEventListener('click', e => {
      const dropdown = document.getElementById('filterDropdown');
      if (dropdown && !dropdown.contains(e.target)) {
        dropdown.classList.remove('open');
      }
    });
    this.initTheme();
    // iOS keyboard visibility: hide bottom fixed elements when keyboard opens
    if ('visualViewport' in window) {
      const onViewportResize = () => {
        const heightDiff = window.innerHeight - window.visualViewport.height;
        document.body.classList.toggle('keyboard-open', heightDiff > 150);
      };
      window.visualViewport.addEventListener('resize', onViewportResize);
    } else {
      document.addEventListener('focusin', (e) => {
        const tag = (e.target?.tagName || '').toLowerCase();
        if (tag === 'input' || tag === 'textarea') document.body.classList.add('keyboard-open');
      });
      document.addEventListener('focusout', () => document.body.classList.remove('keyboard-open'));
    }
    // Pause/resume polling when tab is hidden/shown
    document.addEventListener('visibilitychange', () => {
      if (document.hidden) {
        if (this.pollingTimer) { clearInterval(this.pollingTimer); this.pollingTimer = null; }
        if (this.progressTimer) { clearInterval(this.progressTimer); this.progressTimer = null; }
      } else {
        if (this.pollingIds.size > 0 && !this.pollingTimer) {
          this.pollingTimer = setInterval(() => this.pollInsightStatus(), 5000);
        }
        if (!this.progressTimer) {
          this.startProgressPolling();
        }
      }
    });
    await this.initAuth();
    this.syncFromURL();
    if (this.tagsPageOpen) {
      this.showTagsPage();
    } else if (this.datesPageOpen) {
      this.showDatesPage();
    } else if (this.insightPageId) {
      await this.showInsightPage(this.insightPageId, this.insightPageSource, true);
    } else {
      await this.load();
    }
    this.startProgressPolling();
    const hasSeenProgress = localStorage.getItem('progressPanelSeen');
    this.progressCollapsed = !!hasSeenProgress;
    if (!hasSeenProgress) {
      localStorage.setItem('progressPanelSeen', '1');
      // Auto-collapse progress panel after 0.5s on first visit only
      setTimeout(() => {
        if (!this.progressCollapsed) {
          this.progressCollapsed = true;
          this.fetchProgress();
        }
      }, 500);
    }
    // Progress bar click to expand
    const progressBar = document.getElementById('progressBar');
    if (progressBar) {
      progressBar.addEventListener('click', () => this.toggleProgressCollapse());
    }
    // Text selection handler for comments / AI revise
    const handleSelectionEnd = (e) => {
      if (!this.insightPageId) return;
      if (!this.isDesktopViewport()) return;
      const touch = e.changedTouches && e.changedTouches[0];
      const cx = touch ? touch.clientX : e.clientX;
      const cy = touch ? touch.clientY : e.clientY;
      // Record position for tooltip placement
      this.mousePos = { x: cx, y: cy };
      // Ignore if clicking inside comment panel or tooltip
      const panel = document.getElementById('insightCommentsPanel');
      const tooltip = document.getElementById('selectionTooltip');
      if (panel && panel.contains(e.target)) return;
      if (tooltip && tooltip.contains(e.target)) return;
      // Handle image click
      if (e.target.tagName === 'IMG') {
        const page = document.getElementById('insightPage');
        if (page && page.contains(e.target)) {
          if (!this.isDesktopViewport()) return;
          this.showImageLightbox(e.target);
          return;
        }
      }
      const sel = window.getSelection();
      if (sel && sel.toString().trim().length > 0) {
        const page = document.getElementById('insightPage');
        if (page && page.contains(sel.anchorNode)) {
          this.pendingSelection = this.extractSelectionContext(sel);
          this.pendingImage = null;
          this.showSelectionTooltip(sel);
          return;
        }
      }
      this.hideSelectionTooltip();
    };
    document.addEventListener('mouseup', handleSelectionEnd);
    // Hide tooltip on scroll (throttled to rAF)
    { let _scrollTicking = false;
    document.addEventListener('scroll', () => {
      if (_scrollTicking) return;
      _scrollTicking = true;
      requestAnimationFrame(() => { this.hideSelectionTooltip(); _scrollTicking = false; });
    }, true); }
    // Mobile text selection: use selectionchange + touchend for iOS reliability
    let mobileSelTimer = null;
    const handleMobileSelection = (source) => {
      if (!this.insightPageId) return;
      if (this.isDesktopViewport()) return;

      clearTimeout(mobileSelTimer);
      mobileSelTimer = setTimeout(() => {
        const sel = window.getSelection();
        const text = sel ? sel.toString().trim() : '';
        if (!text.length) {
          this.hideSelectionTooltip();
          return;
        }
        const anchor = sel.anchorNode;
        if (!anchor) return;

        const panel = document.getElementById('insightCommentsPanel');
        const tooltip = document.getElementById('selectionTooltip');
        const page = document.getElementById('insightPage');
        if (panel && panel.contains(anchor)) return;
        if (tooltip && tooltip.contains(anchor)) return;
        if (!page || !page.contains(anchor)) {
          this.hideSelectionTooltip();
          return;
        }

        this.pendingSelection = this.extractSelectionContext(sel);
        this.pendingImage = null;
        this.showSelectionTooltip(sel);
      }, 50);
    };
    document.addEventListener('selectionchange', () => handleMobileSelection('selectionchange'));
    // Swipe hint indicator element (created once, reused)
    this._ensureSwipeHint = () => {
      if (document.getElementById('swipeHint')) return;
      const el = document.createElement('div');
      el.id = 'swipeHint';
      el.innerHTML = '<svg viewBox="0 0 24 24"><line x1="4" y1="12" x2="20" y2="12" class="arrow-body"/><polyline points="14 6 20 12 14 18" class="arrow-head"/></svg><span class="hint-label"></span>';
      document.body.appendChild(el);
    };
    this._updateSwipeHint = (dir, progress) => {
      this._ensureSwipeHint();
      const el = document.getElementById('swipeHint');
      if (!el) return;
      el.classList.remove('visible', 'ready');
      if (!dir) return;
      // dir: 'next' (swipe left → go next → right arrow), 'prev' (swipe right → go prev → left arrow)
      el.querySelector('svg').style.transform = dir === 'next' ? '' : 'scaleX(-1)';
      el.querySelector('.hint-label').textContent = dir === 'next' ? '下一篇' : '上一篇';
      if (progress >= 1) {
        el.classList.add('ready');
      } else if (progress > 0.15) {
        el.classList.add('visible');
      }
    };
    this._hideSwipeHint = () => {
      const el = document.getElementById('swipeHint');
      if (el) el.classList.remove('visible', 'ready');
    };

    // Record touch position early so selectionchange can use it even if touchend fires later
    const _insideBody = (target) => {
      const body = document.getElementById('insightPageBody');
      return body && body.contains(target);
    };
    document.addEventListener('touchstart', (e) => {
      if (!this.insightPageId) return;
      if (this.isDesktopViewport()) return;
      if (!_insideBody(e.target)) { delete this._swipeStart; return; }
      const touch = e.changedTouches[0];
      this._touchPos = { x: touch.clientX, y: touch.clientY };
      this._swipeStart = { x: touch.clientX, y: touch.clientY, t: Date.now() };
    }, { passive: true });
    document.addEventListener('touchmove', (e) => {
      if (!this._swipeStart || this._swipeHintActive) return;
      if (this.insightPageEditing) return;
      // Throttle to one frame (passive handler, non-blocking)
      if (this._swipeRafId) return;
      this._swipeRafId = requestAnimationFrame(() => { this._swipeRafId = null; });
      if (!_insideBody(e.target)) return;
      const tag = (e.target?.tagName || '').toLowerCase();
      if (tag === 'input' || tag === 'textarea' || tag === 'select') return;
      const touch = e.changedTouches[0];
      const dx = touch.clientX - this._swipeStart.x;
      const dy = touch.clientY - this._swipeStart.y;
      const absDx = Math.abs(dx);
      const absDy = Math.abs(dy);
      // Only show hint when horizontal dominates and has meaningful distance
      if (absDx > 30 && absDx > absDy * 1.5) {
        const hasSelection = !!window.getSelection().toString().trim().length;
        if (hasSelection) return;
        // dx>0: swipe right → previous paper
        const dir = dx > 0 ? 'prev' : 'next';
        const progress = Math.min(absDx / 50, 1);
        this._updateSwipeHint(dir, progress);
      } else {
        this._hideSwipeHint();
      }
    }, { passive: true });
    document.addEventListener('touchend', (e) => {
      if (!this.insightPageId) return;
      if (this.isDesktopViewport()) return;
      const tooltip = document.getElementById('selectionTooltip');
      if (tooltip && tooltip.contains(e.target)) return;
      const touch = e.changedTouches[0];
      this._touchPos = { x: touch.clientX, y: touch.clientY };

      // Swipe navigation on mobile
      const ss = this._swipeStart;
      if (ss && !this.insightPageEditing) {
        const tag = (e.target?.tagName || '').toLowerCase();
        const skip = tag === 'input' || tag === 'textarea' || tag === 'select';
        if (!skip && !e.target.closest('#insightCommentsPanel') && !e.target.closest('#mobileCommentEditor') && !e.target.closest('#mobileSelectionBar')) {
          const dx = touch.clientX - ss.x;
          const dy = touch.clientY - ss.y;
          const dt = Date.now() - ss.t;
          const absDx = Math.abs(dx);
          const absDy = Math.abs(dy);
          // Skip swipe if user has text selected (selecting / adjusting selection).
          const hasSelection = !!window.getSelection().toString().trim().length;
          if (absDx > 50 && absDx > absDy * 1.5 && dt < 500 && !hasSelection) {
            if (dx > 0 && this.insightNeighbors.prev) {
              this.showInsightPage(this.insightNeighbors.prev.id, this.insightNeighbors.prev.source_type);
              this._hideSwipeHint();
              delete this._swipeStart;
              return;
            } else if (dx < 0 && this.insightNeighbors.next) {
              this.showInsightPage(this.insightNeighbors.next.id, this.insightNeighbors.next.source_type);
              this._hideSwipeHint();
              delete this._swipeStart;
              return;
            }
          }
        }
      }
      delete this._swipeStart;
      this._hideSwipeHint();

      // Delay to let the system finish updating the selection
      setTimeout(() => handleMobileSelection('touchend'), 400);
    });
    // Tap on paper title to auto-select heading and show comment bar
    document.addEventListener('click', (e) => {
      if (!this.insightPageId) return;
      const headerInner = e.target.closest('.insight-page-header-inner');
      if (!headerInner) return;
      if (e.target.closest('.insight-page-close')) return;
      if (e.target.closest('.insight-meta-toggle')) return;
      const h2 = headerInner.querySelector('h2');
      if (!h2) return;
      const range = document.createRange();
      range.selectNodeContents(h2);
      const sel = window.getSelection();
      sel.removeAllRanges();
      sel.addRange(range);
      this.pendingSelection = this.extractSelectionContext(sel);
      this.pendingImage = null;
      this.showSelectionTooltip(sel);
    });
    // Global resize handler: ensure .container stays correctly centered on any page
    window.addEventListener('resize', () => this.adjustLayoutForComments());
  },
};
