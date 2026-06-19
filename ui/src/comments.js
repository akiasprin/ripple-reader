// Auto-extracted from the original monolithic app.js.
// Each module exports a slice of the global `app` object; main.js merges them.
export const comments = {
  buildHeadingPrompt() {
    return 'Step1:在脑中思考要提取的高价值内容[梳理项]，包括但不限于：关键洞察、反直觉发现、创造性思路或方法、核心结论、重要定义或定理等等。如果原文同时包含多种类型，请按类型分别梳理，不要把洞察和反直觉发现混在一起讨论。Step2:对每个[梳理项]进行两项输出：1.「详讲」:极度详细的、包含原文公式的讲解；讲解完成后，整段输出自我评估（不要放在详讲的分点里），判断该讲解是否全面、有无重要遗漏。2.「类比」:使用最贴切的类比场景对全面详讲内容进行解释，指出该类比本身的局限，明确说明在哪些条件/假设下这个类比不再成立或可能误导理解。Step3:严格按格式输出：开头先写一句友好开场，说明梳理项总数。然后每个[梳理项]独立成块，依次包含小标题、①「详讲」、②「类比」三个部分；小标题固定格式「梳理项 N：类型：内容」，例如「梳理项 1：重要定义与定理：缩放定律」；每部分内部用编号条目分点呈现，避免连续大段文字；「类比」固定分为三点：场景设定、逻辑映射、局限性说明；「详讲」和「类比」中的核心概念、关键结论、重要条件等重点内容使用 Markdown 加粗（**文本**）突出显示';
  },
  onAddCommentClick() {
    this.hideSelectionTooltip();
    window.getSelection().removeAllRanges();
    if (!this.isDesktopViewport()) {
      this.showMobileCommentEditor('manual');
      return;
    }
    this.showCommentsPanel();
    if (this.pendingImage) {
      this.renderImageCommentForm(this.pendingImage);
    } else if (this.pendingSelection) {
      this.renderCommentForm(this.pendingSelection);
    }
  },

  isDesktopViewport() {
    const mq = window.getComputedStyle(document.body, '::before').content;
    return mq === '"desktop"';
  },

  adjustLayoutForComments() {
    const container = document.querySelector('.container');
    if (!container) return;
    if (!this.insightPageId || !this.isDesktopViewport()) {
      container.style.marginLeft = '';
      container.style.marginRight = '';
      container.style.maxWidth = '';
      return;
    }
    const panelWidth = 320;
    const gap = 20;
    const maxW = 880;
    const available = window.innerWidth - (this.commentsPanelOpen ? panelWidth : 0);
    const w = Math.min(maxW, available - gap * 2);
    const ml = Math.max(gap, (available - w) / 2);
    container.style.marginLeft = ml + 'px';
    container.style.marginRight = 'auto';
    container.style.maxWidth = w + 'px';
  },

  showCommentsPanel() {
    const alreadyOpen = this.commentsPanelOpen;
    this.commentsPanelOpen = true;
    const panel = document.getElementById('insightCommentsPanel');
    if (panel) panel.classList.add('open');
    const toggle = document.getElementById('commentsToggleBtn');
    if (toggle) toggle.classList.remove('show');
    this.adjustLayoutForComments();
    // On mobile, hide floating bars so they don't overlap with comments.
    // Only save state on first open — re-entering while already open would
    // overwrite the saved state with "hidden" and prevent restoring later.
    if (!this.isDesktopViewport() && !alreadyOpen) {
      const tocBar = document.getElementById('insightTocBar');
      const tocPanel = document.getElementById('insightTOC');
      const progressBar = document.getElementById('progressBar');
      const progressPanel = document.getElementById('insightProgressPanel');
      if (tocBar) { this._tocBarWasShown = tocBar.classList.contains('show'); tocBar.classList.remove('show'); }
      if (tocPanel) { this._tocPanelWasOpen = tocPanel.classList.contains('show'); tocPanel.classList.remove('show'); }
      if (progressBar) { this._progressBarWasShown = progressBar.classList.contains('show'); progressBar.classList.remove('show'); }
      if (progressPanel) { this._progressPanelWasOpen = progressPanel.classList.contains('show'); progressPanel.classList.remove('show'); }
    }
  },

  hideCommentsPanel(skipLayoutAdjust = false) {
    this.commentsPanelOpen = false;
    const panel = document.getElementById('insightCommentsPanel');
    if (panel) panel.classList.remove('open');
    this.activeCommentId = null;
    this.updateHighlightActive();
    // Show toggle button only when there are annotations to reopen.
    if (this.insightPageId && this.comments.length > 0) {
      const toggle = document.getElementById('commentsToggleBtn');
      if (toggle) toggle.classList.add('show');
    }
    if (!skipLayoutAdjust) {
      this.adjustLayoutForComments();
    }
    // Restore floating bars on mobile that were hidden when comments opened
    if (!this.isDesktopViewport()) {
      const tocBar = document.getElementById('insightTocBar');
      const tocPanel = document.getElementById('insightTOC');
      const progressBar = document.getElementById('progressBar');
      const progressPanel = document.getElementById('insightProgressPanel');
      if (tocBar && this._tocBarWasShown) tocBar.classList.add('show');
      if (tocPanel && this._tocPanelWasOpen) tocPanel.classList.add('show');
      if (progressBar && this._progressBarWasShown) progressBar.classList.add('show');
      if (progressPanel && this._progressPanelWasOpen) progressPanel.classList.add('show');
      delete this._tocBarWasShown;
      delete this._tocPanelWasOpen;
      delete this._progressBarWasShown;
      delete this._progressPanelWasOpen;
    }
  },

  async loadComments(paperId, autoOpen = false) {
    // Close and reopen bubble only when navigating to a different paper.
    // Same-paper polls must leave the open bubble alone.
    let reopenBubble = false;
    const bubbleFromOtherPaper = this._openReplyBubbleId != null
      && this._openReplyBubblePaperId !== paperId;
    if (bubbleFromOtherPaper) {
      this.hideReplyBubble();
      reopenBubble = true;
    }
    const hadBubble = !!document.getElementById('replyBubble')
      && !this._openReplyBubbleId;
    if (hadBubble) {
      this.hideReplyBubble();
      reopenBubble = true;
    }

    let loaded = false;
    try {
      const res = await fetch('/api/papers/' + encodeURIComponent(this.paperSource(paperId)) + '/' + encodeURIComponent(paperId) + '/comments?_=' + Date.now(), { cache: 'no-store' });
      if (!res.ok) throw new Error('failed');
      const data = await res.json();
      if (!this.isCurrentInsight(paperId)) return false;
      this.comments = data;
      loaded = true;
    } catch (e) {
      // Don't clear comments on fetch failure — background tabs may have
      // fetch requests throttled/cancelled by the browser. Preserving old
      // data keeps the poll alive until a successful refresh.
      console.warn('[loadComments] fetch failed, keeping existing comments:', e);
    }
    this.renderComments();
    this.renderHighlights();
    if (!loaded) return false;
    if (autoOpen && this.comments.length > 0 && !this.commentsPanelOpen && this.isDesktopViewport()) {
      this.showCommentsPanel();
    }
    // Restore reply bubble from URL (survives page refresh)
    const replyId = new URL(window.location.href).searchParams.get('reply');
    if (replyId && !document.getElementById('replyBubble')) {
      const comment = this.comments.find(c => c.id === parseInt(replyId, 10));
      if (comment && comment.ai_reply) {
        setTimeout(() => this.showReplyModal(comment.id, null), 300);
      }
    }
    // If bubble was open on previous paper, auto-open first AI reply on new paper
    if (reopenBubble) {
      const firstAi = this.comments.find(c => c.is_ai && c.ai_reply && c.ai_status !== 'pending');
      if (firstAi) {
        setTimeout(() => this.showReplyModal(firstAi.id, null), 500);
      }
    }
    return true;
  },

  renderComments() {
    const list = document.getElementById('commentsList');
    const countEl = document.getElementById('commentsCount');
    const formArea = document.getElementById('commentFormArea');
    const toggleCount = document.getElementById('commentsToggleCount');
    if (!list) return;

    // Skip DOM rebuild if nothing meaningful changed — prevents text selection
    // from being destroyed by polling-driven re-renders.
    // Note: activeCommentId is excluded from fingerprint; the active class is
    // toggled via direct DOM manipulation to avoid full re-renders on click.
    const fp = this.comments.map(c => {
      // During pending, ai_reply grows on backend but isn't rendered (only
      // a spinner is shown), so ignore ai_reply length changes to avoid
      // re-rendering the entire list every 3 seconds during polling.
      const aiReplyLen = c.ai_status === 'pending' ? 'P' : (c.ai_reply ? c.ai_reply.length : 0);
      return c.id + ':' + c.ai_status + ':' + aiReplyLen;
    }).join(',') + '|auth:' + !!this.authenticated;
    if (fp === this._commentsFingerprint && this.comments.length > 0) {
      this._updateActiveCommentClass();
      return;
    }
    this._commentsFingerprint = fp;

    if (countEl) countEl.textContent = this.comments.length;
    if (toggleCount) toggleCount.textContent = this.comments.length;
    if (this.comments.length === 0) {
      list.innerHTML = '<div class="empty-comments">暂无批注<br>选中正文添加批注</div>';
      if (formArea && !formArea.querySelector('textarea')) formArea.innerHTML = '';
      this._commentFingerprints = null;
    } else {
      // Save active form state before any DOM manipulation
      const savedForms = this._saveActiveForms();

      const lastId = this.lastAddedCommentId;
      const childrenMap = new Map();
      for (const c of this.comments) {
        if (c.parent_id) {
          if (!childrenMap.has(c.parent_id)) childrenMap.set(c.parent_id, []);
          childrenMap.get(c.parent_id).push(c);
        }
      }

      const roots = this.comments.filter(c => !c.parent_id);

      // Compute per-group fingerprints for incremental updates
      const newGroupFps = new Map();
      for (const r of roots) {
        newGroupFps.set(r.id, this._computeGroupFingerprint(r, childrenMap));
      }

      // Incremental update when possible, full rebuild otherwise
      if (this._commentFingerprints && this._commentFingerprints.size > 0 && list.querySelector('.comment-card')) {
        this._incrementalUpdateComments(list, roots, childrenMap, newGroupFps, lastId);
      } else {
        list.innerHTML = roots.map(c => this._renderCommentCard(c, childrenMap, lastId)).join('');
        if (lastId !== null) {
          setTimeout(() => { this.lastAddedCommentId = null; }, 500);
        }
      }

      this._commentFingerprints = newGroupFps;

      // Only clear form area if user is NOT actively typing
      if (formArea && !formArea.querySelector('textarea')) {
        formArea.innerHTML = '';
      }

      // Restore any forms that were active before the DOM update
      this._restoreActiveForms(savedForms);
    }
    // Update panel/toggle visibility based on current state
    const panel = document.getElementById('insightCommentsPanel');
    const toggle = document.getElementById('commentsToggleBtn');
    if (this.comments.length === 0) {
      if (panel) panel.classList.remove('open');
      if (toggle) toggle.classList.remove('show');
    } else if (panel) {
      if (this.commentsPanelOpen) {
        panel.classList.add('open');
        if (toggle) toggle.classList.remove('show');
      } else {
        panel.classList.remove('open');
        if (toggle) toggle.classList.add('show');
      }
    }
    // After DOM update, check which expand buttons are actually needed
    // (CSS clamp may not overflow for short text).
    requestAnimationFrame(() => {
      this._revealExpandButtons();
    });
  },

  showReplyModal(id, event) {
    const comment = this.comments.find(c => c.id === id);
    // Get the full text: AI replies use ai_reply, regular comments use comment
    const fullText = comment?.ai_reply || comment?.comment;
    if (!comment || !fullText) return;

    // If the same reply is already open in immersive mode, minimize it
    // instead of rebuilding — prevents Space/Enter re-trigger race.
    if (this._openReplyBubbleId === id) {
      const existing = document.getElementById('replyBubble');
      if (existing && existing.classList.contains('immersive')) {
        const btn = event?.target;
        this.exitImmersiveMode();
        if (btn) btn.blur();
        return;
      }
    }

    // Close any existing bubble first (this also clears _openReplyBubbleId)
    this.hideReplyBubble();
    // Clear pending cleanup timer and force-remove residual DOM so the
    // old bubble cannot collide with the new one during cleanup timeout.
    if (this._replyBubbleCleanupTimer) {
      clearTimeout(this._replyBubbleCleanupTimer);
      this._replyBubbleCleanupTimer = null;
    }
    const residualBubble = document.getElementById('replyBubble');
    if (residualBubble) residualBubble.remove();
    const residualOverlay = document.getElementById('replyBubbleOverlay');
    if (residualOverlay) residualOverlay.remove();
    // Track which reply is open so polling refreshes don't destroy it.
    // Must be set AFTER hideReplyBubble() which sets _openReplyBubbleId = null.
    this._openReplyBubbleId = id;
    this._openReplyBubblePaperId = this.insightPageId;

    const bubble = document.createElement('div');
    bubble.id = 'replyBubble';
    bubble.className = 'reply-bubble';
    bubble.innerHTML = `
      <div class="reply-bubble-drag-handle">
        <span class="reply-bubble-grip"><i></i><i></i><i></i></span>
        <span class="reply-bubble-handle-text"></span>
        <div class="reply-bubble-actions">
          <button class="reply-bubble-expand-btn" title="放大">↗</button>
          <button class="reply-bubble-close-btn" title="关闭">×</button>
        </div>
      </div>
      <div class="reply-bubble-inner"></div>
      <div class="reply-bubble-resize-handle"></div>
    `;
    document.body.appendChild(bubble);

    const inner = bubble.querySelector('.reply-bubble-inner');
    inner.innerHTML = this.renderMarkdown(this.escapeHtml(fullText));

    // Show comment text in drag handle (visible in immersive mode)
    const handleText = bubble.querySelector('.reply-bubble-handle-text');
    if (handleText) {
      // For AI comments, show the user's original comment; for regular
      // comments, show the quote or first few words.
      const handleSrc = comment.comment || comment.quote || '';
      if (handleSrc) handleText.textContent = handleSrc;
    }
    if (typeof Prism !== 'undefined') {
      inner.querySelectorAll('pre code[class^="language-"]').forEach(block => {
        if (!block.dataset.prismHighlight) {
          Prism.highlightElement(block);
          block.dataset.prismHighlight = '1';
        }
      });
    }

    // Position near button if event provided; otherwise center and enter immersive
    let btn = null;
    if (event && event.target) {
      btn = event.target;
      const rect = btn.getBoundingClientRect();
      const bubbleWidth = Math.min(420, window.innerWidth - 40);
      bubble.style.maxWidth = bubbleWidth + 'px';
      bubble.style.visibility = 'hidden';
      const bubbleRect = bubble.getBoundingClientRect();
      const bubbleHeight = bubbleRect.height;
      bubble.style.visibility = '';

      let left = rect.left - bubbleWidth - 12;
      if (left < 10) left = rect.right + 12;
      if (left + bubbleWidth > window.innerWidth - 10) left = window.innerWidth - bubbleWidth - 10;

      let top = rect.top + (rect.height / 2) - (bubbleHeight / 2);
      if (top < 10) top = 10;
      if (top + bubbleHeight > window.innerHeight - 10) top = window.innerHeight - bubbleHeight - 10;

      bubble.style.left = left + 'px';
      bubble.style.top = top + 'px';

      this._replyBubbleOriginalRect = { left, top, maxWidth: bubbleWidth + 'px' };
      this._replyBubbleBtnRect = rect;
    } else {
      // Restore from sessionStorage — center briefly then enter immersive
      const vw = window.innerWidth;
      const bw = Math.min(420, vw - 40);
      bubble.style.maxWidth = bw + 'px';
      bubble.style.left = Math.max(10, (vw - bw) / 2) + 'px';
      bubble.style.top = '60px';
    }

    // Expand button handler
    const expandBtn = bubble.querySelector('.reply-bubble-expand-btn');
    if (expandBtn) {
      expandBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        if (bubble.classList.contains('immersive')) {
          this.exitImmersiveMode();
        } else {
          this.enterImmersiveMode();
        }
      });
    }

    // Close button handler
    const closeBtn = bubble.querySelector('.reply-bubble-close-btn');
    if (closeBtn) {
      closeBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        this.hideReplyBubble();
      });
    }

    // Drag handler
    const dragHandle = bubble.querySelector('.reply-bubble-drag-handle');
    if (dragHandle) {
      this._setupBubbleDrag(bubble, dragHandle);
    }

    // Resize handler (immersive only)
    const resizeHandle = bubble.querySelector('.reply-bubble-resize-handle');
    if (resizeHandle) {
      this._setupBubbleResize(bubble, resizeHandle);
    }

    // Use a single stable handler (not a per-bubble closure) so double-clicks
    // cannot leave zombie listeners behind. The handler checks the live DOM.
    if (!this._replyBubbleClickHandler) {
      this._replyBubbleClickHandler = (e) => {
        const b = document.getElementById('replyBubble');
        if (!b || !this._openReplyBubbleId) return;
        if (!b.contains(e.target) && e.target !== this._replyBubbleOpenBtn) {
          this.hideReplyBubble();
        }
      };
    }
    this._replyBubbleOpenBtn = btn;
    requestAnimationFrame(() => {
      document.addEventListener('click', this._replyBubbleClickHandler);
      // Always enter immersive — user explicitly requested full content view
      this.enterImmersiveMode();
    });

    // Persist bubble state in URL so it survives refresh
    const url = new URL(window.location.href);
    url.searchParams.set('reply', id);
    history.pushState({ replyBubble: true }, '', url);
    this._replyBubblePopHandler = () => {
      if (document.getElementById('replyBubble')) {
        this.hideReplyBubble();
      }
    };
    window.addEventListener('popstate', this._replyBubblePopHandler);
    // Blur source button so keyboard (Space/Enter) targets bubble controls,
    // not the external button that opened this bubble.
    if (btn) btn.blur();
  },

  enterImmersiveMode() {
    const bubble = document.getElementById('replyBubble');
    if (!bubble || bubble.classList.contains('immersive')) return;

    // Clean up any stale overlay from rapid toggle
    const staleOverlay = document.getElementById('replyBubbleOverlay');
    if (staleOverlay) {
      staleOverlay.remove();
      if (this._overlayClickHandler) {
        staleOverlay.removeEventListener('click', this._overlayClickHandler);
        delete this._overlayClickHandler;
      }
    }
    if (this._escHandler) {
      document.removeEventListener('keydown', this._escHandler);
      delete this._escHandler;
    }

    // Save current rect before transitioning (must be string for style assignment)
    const rect = bubble.getBoundingClientRect();
    this._replyBubbleOriginalRect = {
      left: rect.left + 'px',
      top: rect.top + 'px',
      maxWidth: bubble.style.maxWidth || '420px',
    };

    // Create overlay (transparent bg + blur only, no darkening)
    const overlay = document.createElement('div');
    overlay.className = 'reply-bubble-overlay';
    overlay.id = 'replyBubbleOverlay';
    document.body.appendChild(overlay);

    // Fade in overlay
    requestAnimationFrame(() => overlay.classList.add('active'));

    // Overlay click to exit
    this._overlayClickHandler = (e) => {
      if (e.target === overlay) {
        this.hideReplyBubble();
      }
    };
    overlay.addEventListener('click', this._overlayClickHandler);

    // ESC key to close the bubble entirely
    this._escHandler = (e) => {
      if (e.key === 'Escape') {
        this.hideReplyBubble();
      }
    };
    document.addEventListener('keydown', this._escHandler);

    // Switch expand button icon
    const expandBtn = bubble.querySelector('.reply-bubble-expand-btn');
    if (expandBtn) {
      expandBtn.textContent = '↙';
      expandBtn.title = '缩小';
    }

    // Apply immersive class and center smoothly via CSS transitions.
    bubble.classList.add('immersive');
    // Clear any prior resize inline styles so CSS class values take effect
    bubble.style.maxWidth = '';
    bubble.style.width = '';
    bubble.style.maxHeight = '';
    bubble.style.height = '';
    const vw = window.innerWidth;
    const isMobile = vw <= 640;
    // Match CSS: .reply-bubble.immersive { width: min(880px, 90vw) }
    const targetWidth = isMobile ? vw * 0.95 : Math.min(880, vw * 0.9);
    const bubbleHeight = isMobile ? window.innerHeight * 0.90 : Math.min(window.innerHeight * 0.75, window.innerHeight - 40);
    bubble.style.left = Math.max(10, (vw - targetWidth) / 2) + 'px';
    bubble.style.top = Math.max(10, (window.innerHeight - bubbleHeight) / 2) + 'px';

    // Darken status bar to match overlay background
    this._updateThemeColorForBubble(true);
  },

  exitImmersiveMode() {
    const bubble = document.getElementById('replyBubble');
    if (!bubble || !bubble.classList.contains('immersive')) return;

    // Remove immersive class
    bubble.classList.remove('immersive');
    bubble.classList.remove('dragging');
    bubble.style.transform = '';

    // Restore original position
    if (this._replyBubbleOriginalRect) {
      bubble.style.left = this._replyBubbleOriginalRect.left;
      bubble.style.top = this._replyBubbleOriginalRect.top;
      bubble.style.maxWidth = this._replyBubbleOriginalRect.maxWidth;
    }

    // Switch expand button icon back
    const expandBtn = bubble.querySelector('.reply-bubble-expand-btn');
    if (expandBtn) {
      expandBtn.textContent = '↗';
      expandBtn.title = '放大';
    }

    // Remove overlay immediately (fade-out handled by CSS transition on removal)
    const overlay = document.getElementById('replyBubbleOverlay');
    if (overlay) {
      overlay.classList.remove('active');
      overlay.remove();
      if (this._overlayClickHandler) {
        overlay.removeEventListener('click', this._overlayClickHandler);
        delete this._overlayClickHandler;
      }
    }

    // Remove ESC handler
    if (this._escHandler) {
      document.removeEventListener('keydown', this._escHandler);
      delete this._escHandler;
    }

    // Restore status bar color
    this._updateThemeColorForBubble(false);
  },

  _updateThemeColorForBubble(isOverlay) {
    // Keep page theme-color unchanged — dimming the status bar looks jarring on iOS
  },

  _setupBubbleDrag(bubble, handle) {
    let isDragging = false;
    let startX = 0;
    let startY = 0;
    let translateX = 0;
    let translateY = 0;
    let savedTransition = '';

    const getClientPos = (e) => {
      const t = e.touches && e.touches[0] ? e.touches[0] : e;
      return { x: t.clientX, y: t.clientY };
    };

    const onStart = (e) => {
      // Mouse: only left click; Touch: always allowed
      if (e.type === 'mousedown' && e.button !== 0) return;
      if (!bubble.classList.contains('immersive')) return;
      // Don't intercept action button taps (allow click to fire on mobile)
      const target = e.target;
      if (target && target.closest && target.closest('.reply-bubble-expand-btn, .reply-bubble-close-btn')) return;
      isDragging = true;
      const p = getClientPos(e);
      startX = p.x - translateX;
      startY = p.y - translateY;
      savedTransition = bubble.style.transition;
      bubble.style.transition = 'none';
      bubble.classList.add('dragging');
      e.preventDefault();
    };

    const onMove = (e) => {
      if (!isDragging) return;
      const p = getClientPos(e);
      translateX = p.x - startX;
      translateY = p.y - startY;

      // Boundary check: keep at least 60px visible on each edge
      const rect = bubble.getBoundingClientRect();
      const minVisible = 60;
      const maxTx = window.innerWidth - minVisible - rect.left;
      const minTx = -rect.right + minVisible;
      const maxTy = window.innerHeight - minVisible - rect.top;
      const minTy = -rect.bottom + minVisible;

      translateX = Math.max(minTx, Math.min(maxTx, translateX));
      translateY = Math.max(minTy, Math.min(maxTy, translateY));

      bubble.style.transform = `translate(${translateX}px, ${translateY}px)`;
    };

    const onEnd = () => {
      if (!isDragging) return;
      isDragging = false;
      bubble.style.transition = savedTransition;
      bubble.classList.remove('dragging');
    };

    handle.addEventListener('mousedown', onStart);
    handle.addEventListener('touchstart', onStart, { passive: false });
    document.addEventListener('mousemove', onMove);
    document.addEventListener('touchmove', onMove, { passive: false });
    document.addEventListener('mouseup', onEnd);
    document.addEventListener('touchend', onEnd);

    // Store cleanup references
    this._bubbleDragCleanup = () => {
      handle.removeEventListener('mousedown', onStart);
      handle.removeEventListener('touchstart', onStart);
      document.removeEventListener('mousemove', onMove);
      document.removeEventListener('touchmove', onMove);
      document.removeEventListener('mouseup', onEnd);
      document.removeEventListener('touchend', onEnd);
    };
  },

  _setupBubbleResize(bubble, handle) {
    let isResizing = false;
    let startX = 0;
    let startY = 0;
    let startWidth = 0;
    let startHeight = 0;
    let savedTransition = '';

    const getClientPos = (e) => {
      const t = e.touches && e.touches[0] ? e.touches[0] : e;
      return { x: t.clientX, y: t.clientY };
    };

    const onStart = (e) => {
      if (e.type === 'mousedown' && e.button !== 0) return;
      if (!bubble.classList.contains('immersive')) return;
      isResizing = true;
      const p = getClientPos(e);
      const rect = bubble.getBoundingClientRect();
      startX = p.x;
      startY = p.y;
      startWidth = rect.width;
      startHeight = rect.height;
      // Kill CSS transitions during resize so the frame follows the cursor instantly
      savedTransition = bubble.style.transition;
      bubble.style.transition = 'none';
      bubble.classList.add('resizing');
      e.preventDefault();
      e.stopPropagation();
    };

    const onMove = (e) => {
      if (!isResizing) return;
      const p = getClientPos(e);
      const dw = p.x - startX;
      const dh = p.y - startY;
      const newW = Math.max(320, Math.min(window.innerWidth - 20, startWidth + dw));
      const newH = Math.max(200, Math.min(window.innerHeight - 20, startHeight + dh));
      bubble.style.width = newW + 'px';
      bubble.style.maxWidth = (window.innerWidth - 20) + 'px';
      bubble.style.maxHeight = newH + 'px';
      bubble.style.height = newH + 'px';
    };

    const onEnd = () => {
      if (!isResizing) return;
      isResizing = false;
      // Restore CSS transitions so expand/collapse animates normally again
      bubble.style.transition = savedTransition;
      bubble.classList.remove('resizing');
    };

    handle.addEventListener('mousedown', onStart);
    handle.addEventListener('touchstart', onStart, { passive: false });
    document.addEventListener('mousemove', onMove);
    document.addEventListener('touchmove', onMove, { passive: false });
    document.addEventListener('mouseup', onEnd);
    document.addEventListener('touchend', onEnd);

    this._bubbleResizeCleanup = () => {
      handle.removeEventListener('mousedown', onStart);
      handle.removeEventListener('touchstart', onStart);
      document.removeEventListener('mousemove', onMove);
      document.removeEventListener('touchmove', onMove);
      document.removeEventListener('mouseup', onEnd);
      document.removeEventListener('touchend', onEnd);
    };
  },

  hideReplyBubble() {
    // Allow loadComments to auto-open a reply again after the bubble is closed
    this._openReplyBubbleId = null;
    this._openReplyBubblePaperId = null;
    const bubble = document.getElementById('replyBubble');

    // Always remove click handler first to prevent double-trigger during animation.
    // Handler is shared across bubbles — remove from listener but keep the reference.
    if (this._replyBubbleClickHandler) {
      document.removeEventListener('click', this._replyBubbleClickHandler);
    }
    this._replyBubbleOpenBtn = null;

    // Clean up back-button handler
    if (this._replyBubblePopHandler) {
      window.removeEventListener('popstate', this._replyBubblePopHandler);
      delete this._replyBubblePopHandler;
    }
    // Remove reply param from URL when bubble closes
    const url = new URL(window.location.href);
    if (url.searchParams.has('reply')) {
      url.searchParams.delete('reply');
      history.replaceState({}, '', url);
    }

    if (bubble && bubble.classList.contains('immersive')) {
      const closingBubble = bubble; // Guard against race: only clean up this element
      const btnRect = this._replyBubbleBtnRect;
      if (btnRect && this.isDesktopViewport()) {
        // Desktop: animate shrink back to the button position
        bubble.classList.remove('immersive');
        bubble.classList.remove('dragging');
        bubble.style.transform = '';
        bubble.style.left = btnRect.left + 'px';
        bubble.style.top = btnRect.top + 'px';
        bubble.style.maxWidth = btnRect.width + 'px';
        bubble.style.maxHeight = btnRect.height + 'px';
        bubble.style.opacity = '0';
        bubble.style.padding = '0px';
      } else {
        // Mobile: shrink + fade out
        bubble.style.transition = 'opacity 0.25s ease-out, transform 0.25s ease-out';
        bubble.style.opacity = '0';
        bubble.style.transform = 'scale(0.85)';
      }
      // Remove overlay immediately
      const overlay = document.getElementById('replyBubbleOverlay');
      if (overlay) {
        overlay.classList.remove('active');
        overlay.remove();
        if (this._overlayClickHandler) {
          overlay.removeEventListener('click', this._overlayClickHandler);
          delete this._overlayClickHandler;
        }
      }
      if (this._escHandler) {
        document.removeEventListener('keydown', this._escHandler);
        delete this._escHandler;
      }
      // Cleanup — animate delay only when shrinking to button
      const cleanup = () => {
        const b = document.getElementById('replyBubble');
        // Only remove if the DOM still holds the same bubble we closed.
        // A new bubble may have been created during the 300ms timeout.
        if (!b || b !== closingBubble) return;
        b.remove();
        if (this._bubbleDragCleanup) {
          this._bubbleDragCleanup();
          delete this._bubbleDragCleanup;
        }
        if (this._bubbleResizeCleanup) {
          this._bubbleResizeCleanup();
          delete this._bubbleResizeCleanup;
        }
        delete this._replyBubbleOriginalRect;
        delete this._replyBubbleBtnRect;
        delete this._replyBubbleCleanupTimer;
        this._updateThemeColorForBubble(false);
      };
      if (btnRect && this.isDesktopViewport()) {
        this._replyBubbleCleanupTimer = setTimeout(cleanup, 300);
      } else {
        // Mobile: wait for exitImmersiveMode transition to finish
        this._replyBubbleCleanupTimer = setTimeout(cleanup, 300);
      }
      return;
    }

    // Non-immersive: immediate removal
    this.exitImmersiveMode();
    if (bubble) bubble.remove();
    const overlay = document.getElementById('replyBubbleOverlay');
    if (overlay) overlay.remove();
    if (this._bubbleDragCleanup) {
      this._bubbleDragCleanup();
      delete this._bubbleDragCleanup;
    }
    if (this._bubbleResizeCleanup) {
      this._bubbleResizeCleanup();
      delete this._bubbleResizeCleanup;
    }
    delete this._replyBubbleOriginalRect;
    delete this._replyBubbleBtnRect;
    this._updateThemeColorForBubble(false);
  },

  hideReplyModal() {
    this.hideReplyBubble();
  },

  toggleAiReply(id) {
    // Delegate to the generic toggle (supports both old aiReply IDs and new commentExpand IDs)
    this.toggleExpandReply(id);
    // Also try legacy IDs for any remaining old-rendered cards
    const short = document.getElementById('aiReplyShort' + id);
    const full = document.getElementById('aiReplyFull' + id);
    const btn = document.getElementById('aiReplyToggle' + id);
    if (short && full && btn) {
      if (full.style.display === 'none') {
        full.style.display = '';
        short.style.display = 'none';
        btn.textContent = '收起';
      } else {
        full.style.display = 'none';
        short.style.display = '';
        btn.textContent = '展开';
      }
    }
  },

  showReplyForm(commentId) {
    const area = document.getElementById('replyFormArea' + commentId);
    if (!area) return;
    const isVisible = area.dataset.visible === 'true';
    // Hide all other reply forms first
    document.querySelectorAll('.reply-form-area').forEach(el => {
      el.innerHTML = '';
      el.dataset.visible = 'false';
    });
    if (isVisible) return;
    area.dataset.visible = 'true';
    area.innerHTML = `
      <div class="reply-form" onclick="event.stopPropagation()">
        <textarea id="replyInput${commentId}" placeholder="输入回复... 以 @AGENT 开头继续向 AGENT 提问" rows="2">@AGENT </textarea>
        <div class="reply-form-actions">
          <button onclick="app.hideReplyForm(${commentId})">取消</button>
          <button class="primary" onclick="app.submitReply(${commentId})">提交</button>
        </div>
      </div>
    `;
    setTimeout(() => {
      const ta = document.getElementById('replyInput' + commentId);
      if (ta) {
        ta.focus();
        ta.setSelectionRange(ta.value.length, ta.value.length);
        // On mobile, ensure the textarea is visible above the keyboard
        if (!this.isDesktopViewport()) {
          setTimeout(() => ta.scrollIntoView({ block: 'center', behavior: 'smooth' }), 250);
        }
      }
    }, 50);
  },

  hideReplyForm(commentId) {
    const area = document.getElementById('replyFormArea' + commentId);
    if (area) {
      area.innerHTML = '';
      area.dataset.visible = 'false';
    }
  },

  async submitReply(commentId) {
    const ta = document.getElementById('replyInput' + commentId);
    const text = ta ? ta.value.trim() : '';
    if (!text) return;
    const parent = this.comments.find(c => c.id === commentId);
    if (!parent) return;
    const isAi = text.startsWith('@AGENT');
    const instruction = isAi ? text.slice(6).trim() : '';
    if (isAi && !instruction) {
      this.showToast('请输入 AI 指令', 'error');
      return;
    }
    try {
      const headers = { 'Content-Type': 'application/json' };
      if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
      if (isAi) {
        const body = {
          quote: parent.quote || '',
          before_ctx: parent.before_ctx || '',
          after_ctx: parent.after_ctx || '',
          instruction: instruction,
          parent_id: commentId,
        };
        const res = await fetch('/api/papers/' + encodeURIComponent(this.insightPageSource) + '/' + encodeURIComponent(this.insightPageId) + '/ai-comments', {
          method: 'POST',
          headers,
          body: JSON.stringify(body),
        });
        if (!res.ok) throw new Error('failed');
        this.hideReplyForm(commentId);
        await this.loadComments(this.insightPageId);
        this.showCommentsPanel();
        this.startAiCommentPoll();
        this.showToast('AI 回复已创建', 'success');
      } else {
        const body = {
          quote: parent.quote || '',
          before_ctx: parent.before_ctx || '',
          after_ctx: parent.after_ctx || '',
          comment: text,
          parent_id: commentId,
        };
        const res = await fetch('/api/papers/' + encodeURIComponent(this.insightPageSource) + '/' + encodeURIComponent(this.insightPageId) + '/comments', {
          method: 'POST',
          headers,
          body: JSON.stringify(body),
        });
        if (!res.ok) throw new Error('failed');
        this.hideReplyForm(commentId);
        await this.loadComments(this.insightPageId);
        this.showToast('回复已添加', 'success');
      }
    } catch (e) {
      console.error('[submitReply]', e);
      this.showToast('发送失败', 'error');
    }
  },

  renderCommentForm(selection) {
    const formArea = document.getElementById('commentFormArea');
    if (!formArea) return;
    const quotePreview = selection ? selection.text : '';
    formArea.innerHTML = `
      <div class="comment-form">
        <div class="comment-form-label">引用</div>
        <div class="comment-form-quote">${this.escape(quotePreview)}</div>
        <textarea id="commentInput" placeholder="输入批注内容..." rows="3">@AGENT </textarea>
        <div class="comment-form-actions">
          <button onclick="app.cancelCommentForm()">取消</button>
          <button class="primary" onclick="app.submitComment()">提交</button>
        </div>
      </div>
    `;
    setTimeout(() => {
      const ta = document.getElementById('commentInput');
      if (ta) { ta.focus(); ta.setSelectionRange(ta.value.length, ta.value.length); }
    }, 50);
  },

  renderImageCommentForm(img) {
    const formArea = document.getElementById('commentFormArea');
    if (!formArea) return;
    const src = img.dataset.fullSrc || img.src;
    formArea.innerHTML = `
      <div class="comment-form">
        <div class="comment-form-label">图片</div>
        <div class="comment-form-image"><img src="${this.escape(src)}" style="max-width:100%;border-radius:8px"></div>
        <textarea id="commentInput" placeholder="输入批注内容..." rows="3">@AGENT </textarea>
        <div class="comment-form-actions">
          <button onclick="app.cancelCommentForm()">取消</button>
          <button class="primary" onclick="app.submitImageComment()">提交</button>
        </div>
      </div>
    `;
    setTimeout(() => {
      const ta = document.getElementById('commentInput');
      if (ta) { ta.focus(); ta.setSelectionRange(ta.value.length, ta.value.length); }
    }, 50);
  },

  cancelCommentForm() {
    const formArea = document.getElementById('commentFormArea');
    if (formArea) formArea.innerHTML = '';
    this.pendingSelection = null;
    this.pendingImage = null;
  },

  showMobileCommentEditor(mode) {
    const overlay = document.getElementById('mobileCommentOverlay');
    const input = document.getElementById('mobileCommentEditorInput');
    const quote = document.getElementById('mobileCommentEditorQuote');
    const title = overlay ? overlay.querySelector('.mobile-comment-editor-title') : null;
    if (!overlay || !input || !quote) return;

    this._mobileEditorMode = mode;

    if (this.pendingImage) {
      const src = this.pendingImage.dataset.fullSrc || this.pendingImage.src;
      quote.innerHTML = '<img src="' + this.escape(src) + '" style="max-width:100%;max-height:160px;border-radius:8px;display:block">';
    } else if (this.pendingSelection) {
      quote.textContent = this.pendingSelection.text;
    } else {
      quote.textContent = '';
    }

    if (mode === 'ai') {
      if (title) title.textContent = 'AI 批注';
      input.value = '@AGENT ';
      input.placeholder = '补充额外的指令...';
    } else {
      if (title) title.textContent = '添加批注';
      input.value = '@AGENT ';
      input.placeholder = '输入批注内容...';
    }

    // Lock body scroll — same approach as insight dialog.
    // On iOS Safari, position:fixed body combined with the soft keyboard
    // causes coordinate-mapping bugs, so we only use overflow:hidden.
    const scrollY = window.scrollY || document.documentElement.scrollTop || 0;
    document.documentElement.style.overflow = 'hidden';
    overlay._savedScrollY = scrollY;

    overlay.classList.add('open');

    // Focus after slide-in animation completes.
    setTimeout(() => {
      input.focus();
      input.setSelectionRange(input.value.length, input.value.length);
    }, 350);
  },

  hideMobileCommentEditor() {
    const overlay = document.getElementById('mobileCommentOverlay');
    if (overlay) {
      overlay.classList.remove('open');
    }
    // Unlock body scroll and restore position.
    document.documentElement.style.overflow = '';
    const scrollY = overlay?._savedScrollY || 0;
    window.scrollTo(0, scrollY);
    if (overlay) delete overlay._savedScrollY;

    document.dispatchEvent(new Event('app:hideMobileEditor'));
    this.hideSelectionTooltip();
    this.cancelCommentForm();
  },

  async submitMobileComment() {
    const input = document.getElementById('mobileCommentEditorInput');
    const text = input ? input.value.trim() : '';
    if (!text) return;
    if (!this.pendingSelection && !this.pendingImage) return;
    if (!this.insightPageId) return;

    if (text.startsWith('@AGENT')) {
      const instruction = text.slice(6).trim();
      if (!instruction) {
        this.showToast('请输入 AI 指令', 'error');
        return;
      }
      const body = {
        quote: this.pendingSelection ? this.pendingSelection.text : '',
        before_ctx: this.pendingSelection ? this.pendingSelection.before : '',
        after_ctx: this.pendingSelection ? this.pendingSelection.after : '',
        instruction: instruction,
      };
      try {
        const headers = { 'Content-Type': 'application/json' };
        if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
        const res = await fetch('/api/papers/' + encodeURIComponent(this.insightPageSource) + '/' + encodeURIComponent(this.insightPageId) + '/ai-comments', {
          method: 'POST', headers, body: JSON.stringify(body),
        });
        if (!res.ok) throw new Error('failed');
        this.pendingSelection = null;
        this.pendingImage = null;
        this.hideMobileCommentEditor();
        await this.loadComments(this.insightPageId);
        this.showCommentsPanel();
        this.startAiCommentPoll();
        this.showToast('AI 批注已创建', 'success');
      } catch (e) {
        console.error('[submitMobileComment] AI', e);
        this.showToast('创建失败', 'error');
      }
      return;
    }

    const body = {
      quote: this.pendingSelection ? this.pendingSelection.text : '',
      before_ctx: this.pendingSelection ? this.pendingSelection.before : '',
      after_ctx: this.pendingSelection ? this.pendingSelection.after : '',
      comment: text,
    };
    try {
      const headers = { 'Content-Type': 'application/json' };
      if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
      const res = await fetch('/api/papers/' + encodeURIComponent(this.insightPageSource) + '/' + encodeURIComponent(this.insightPageId) + '/comments', {
        method: 'POST', headers, body: JSON.stringify(body),
      });
      if (!res.ok) throw new Error('failed');
      const newComment = await res.json();
      this.lastAddedCommentId = newComment.id;
      this.pendingSelection = null;
      this.pendingImage = null;
      this.hideMobileCommentEditor();
      await this.loadComments(this.insightPageId);
      this.showCommentsPanel();
      this.showToast('批注已添加', 'success');
    } catch (e) {
      console.error('[submitMobileComment]', e);
      this.showToast('添加失败', 'error');
    }
  },

  async submitComment() {
    if (!this.pendingSelection || !this.insightPageId) return;
    const ta = document.getElementById('commentInput');
    const text = ta ? ta.value.trim() : '';
    if (!text) return;
    if (text.startsWith('@AGENT')) {
      const instruction = text.slice(6).trim();
      if (!instruction) {
        this.showToast('请输入 AI 指令', 'error');
        return;
      }
      const body = {
        quote: this.pendingSelection.text,
        before_ctx: this.pendingSelection.before,
        after_ctx: this.pendingSelection.after,
        instruction: instruction,
      };
      try {
        const headers = { 'Content-Type': 'application/json' };
        if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
        const res = await fetch('/api/papers/' + encodeURIComponent(this.insightPageSource) + '/' + encodeURIComponent(this.insightPageId) + '/ai-comments', {
          method: 'POST',
          headers,
          body: JSON.stringify(body),
        });
        if (!res.ok) throw new Error('failed');
        this.cancelCommentForm();
        await this.loadComments(this.insightPageId);
        this.showCommentsPanel();
        this.startAiCommentPoll();
        this.showToast('AI 批注已创建', 'success');
      } catch (e) {
        console.error('[submitComment] AI', e);
        this.showToast('创建失败', 'error');
      }
      return;
    }
    const body = {
      quote: this.pendingSelection.text,
      before_ctx: this.pendingSelection.before,
      after_ctx: this.pendingSelection.after,
      comment: text,
    };
    try {
      const headers = { 'Content-Type': 'application/json' };
      if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
      const res = await fetch('/api/papers/' + encodeURIComponent(this.insightPageSource) + '/' + encodeURIComponent(this.insightPageId) + '/comments', {
        method: 'POST',
        headers,
        body: JSON.stringify(body),
      });
      if (!res.ok) throw new Error('failed');
      const newComment = await res.json();
      this.lastAddedCommentId = newComment.id;
      this.cancelCommentForm();
      await this.loadComments(this.insightPageId);
      this.showToast('批注已添加', 'success');
    } catch (e) {
      console.error('[submitComment]', e);
      this.showToast('添加失败', 'error');
    }
  },

  async submitImageComment() {
    if (!this.pendingImage || !this.insightPageId) return;
    const ta = document.getElementById('commentInput');
    const text = ta ? ta.value.trim() : '';
    if (!text) return;
    const src = this.pendingImage.dataset.fullSrc || this.pendingImage.src;
    const body = {
      quote: src,
      before_ctx: '',
      after_ctx: '',
      comment: text,
    };
    try {
      const headers = { 'Content-Type': 'application/json' };
      if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
      const res = await fetch('/api/papers/' + encodeURIComponent(this.insightPageSource) + '/' + encodeURIComponent(this.insightPageId) + '/comments', {
        method: 'POST',
        headers,
        body: JSON.stringify(body),
      });
      if (!res.ok) throw new Error('failed');
      const newComment = await res.json();
      this.lastAddedCommentId = newComment.id;
      this.cancelCommentForm();
      await this.loadComments(this.insightPageId);
      this.showToast('批注已添加', 'success');
    } catch (e) {
      console.error('[submitImageComment]', e);
      this.showToast('添加失败', 'error');
    }
  },

  async deleteComment(id) {
    if (!confirm('确定删除这条批注？')) return;
    try {
      const headers = {};
      if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
      const res = await fetch('/api/comments/' + id, { method: 'DELETE', headers });
      if (!res.ok) throw new Error('failed');
      this.activeCommentId = null;
      if (this.insightPageId) {
        await this.loadComments(this.insightPageId);
      }
      this.showToast('已删除', 'success');
    } catch (e) {
      console.error('[deleteComment]', e);
      this.showToast('删除失败', 'error');
    }
  },

  onAiReviseClick() {
    this.hideSelectionTooltip();
    window.getSelection().removeAllRanges();
    this.submitAiComment();
  },

  async submitAiComment() {
    if (!this.pendingSelection || !this.insightPageId) return;
    const isHeading = this.pendingSelection.isHeading;
    const instruction = isHeading
      ? this.buildHeadingPrompt()
      : '使用示例分步演示、比喻、... 的等方式，协助我理解透彻';
    const body = {
      quote: this.pendingSelection.text,
      before_ctx: this.pendingSelection.before,
      after_ctx: this.pendingSelection.after,
      instruction: instruction,
      is_heading: isHeading,
    };
    try {
      const headers = { 'Content-Type': 'application/json' };
      if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
      const res = await fetch('/api/papers/' + encodeURIComponent(this.insightPageSource) + '/' + encodeURIComponent(this.insightPageId) + '/ai-comments', {
        method: 'POST',
        headers,
        body: JSON.stringify(body),
      });
      if (!res.ok) throw new Error('failed');
      this.pendingSelection = null;
      this.cancelCommentForm();
      await this.loadComments(this.insightPageId);
      this.showCommentsPanel();
      this.startAiCommentPoll();
      this.showToast('AI 批注已创建', 'success');
    } catch (e) {
      console.error('[submitAiComment]', e);
      this.showToast('创建失败', 'error');
    }
  },

  startAiCommentPoll() {
    // Use recursive setTimeout instead of setInterval so callbacks never
    // overlap. This prevents a slow/stale fetch from overwriting a newer
    // result or from clearing the timer while another fetch is in flight.
    if (this._aiPollTimer) {
      clearTimeout(this._aiPollTimer);
      this._aiPollTimer = null;
    }
    const poll = async () => {
      if (!this.insightPageId) {
        this._aiPollTimer = null;
        return;
      }
      // Remember whether we expected a pending reply before this fetch; if the
      // fetch fails we can decide whether to keep polling.
      const hadPending = this.comments.some(
        (c) => c.is_ai && c.ai_status === 'pending'
      );
      const loaded = await this.loadComments(this.insightPageId);
      if (!loaded || !this.insightPageId) {
        // Keep trying only while we know there is something to wait for.
        if (hadPending) {
          this._aiPollTimer = setTimeout(poll, 3000);
        } else {
          this._aiPollTimer = null;
        }
        return;
      }
      const hasPending = this.comments.some(
        (c) => c.is_ai && c.ai_status === 'pending'
      );
      if (!hasPending) {
        this._aiPollTimer = null;
        return;
      }
      this._aiPollTimer = setTimeout(poll, 3000);
    };
    this._aiPollTimer = setTimeout(poll, 3000);
  },

  async applyAiComment(id) {
    try {
      const headers = {};
      if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
      const res = await fetch('/api/comments/' + id + '/apply', { method: 'POST', headers });
      if (!res.ok) {
        let msg = '接受失败';
        if (res.status === 409) {
          try {
            const err = await res.json();
            if (err && err.reason) msg = err.reason;
          } catch (_) {}
        }
        this.showToast(msg, 'error');
        return;
      }
      await this.loadComments(this.insightPageId);
      this.renderInsightPage();
      this.showToast('已接受修改', 'success');
    } catch (e) {
      console.error('[applyAiComment]', e);
      this.showToast('接受失败', 'error');
    }
  },

  async rejectAiComment(id) {
    try {
      const headers = {};
      if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
      const res = await fetch('/api/comments/' + id + '/reject', { method: 'POST', headers });
      if (!res.ok) throw new Error('failed');
      await this.loadComments(this.insightPageId);
      this.showToast('已拒绝', 'success');
    } catch (e) {
      console.error('[rejectAiComment]', e);
      this.showToast('操作失败', 'error');
    }
  },

  async regenerateAiComment(id, event) {
    if (event) event.stopPropagation();
    if (!confirm('确定要重新生成这条 AI 回复吗？')) return;
    try {
      const headers = { 'Content-Type': 'application/json' };
      if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
      const res = await fetch('/api/comments/' + id + '/regenerate', { method: 'POST', headers });
      if (!res.ok) throw new Error('failed');
      await this.loadComments(this.insightPageId);
      this.startAiCommentPoll();
      this.showToast('正在重新生成...', 'success');
    } catch (e) {
      console.error('[regenerateAiComment]', e);
      this.showToast('重新生成失败', 'error');
    }
  },

  onCommentCardClick(id) {
    // Toggle active class via direct DOM manipulation instead of
    // triggering a full re-render just to change a CSS class.
    const list = document.getElementById('commentsList');
    if (list) {
      const oldActive = list.querySelector('.comment-card.active');
      if (oldActive) oldActive.classList.remove('active');
    }
    this.activeCommentId = id;
    this.updateHighlightActive();
    if (list) {
      const newActive = list.querySelector('.comment-card[data-comment-id="' + id + '"]');
      if (newActive) newActive.classList.add('active');
    }
    this.scrollToHighlight(id);
  },

  onCommentCardHover(id) {
    const highlights = document.querySelectorAll('.comment-highlight[data-comment-id="' + id + '"]');
    highlights.forEach(el => el.classList.add('active'));
    const imgHighlights = document.querySelectorAll('img.comment-highlight-img[data-comment-id="' + id + '"]');
    imgHighlights.forEach(el => el.classList.add('active'));
  },

  onCommentCardLeave() {
    document.querySelectorAll('.comment-highlight.active').forEach(el => {
      if (parseInt(el.dataset.commentId, 10) !== this.activeCommentId) {
        el.classList.remove('active');
      }
    });
    document.querySelectorAll('img.comment-highlight-img.active').forEach(el => {
      if (parseInt(el.dataset.commentId, 10) !== this.activeCommentId) {
        el.classList.remove('active');
      }
    });
  },

  scrollToComment(id) {
    const card = document.querySelector('.comment-card[data-comment-id="' + id + '"]');
    if (card) {
      card.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
    }
  },

  // --- Incremental rendering helpers ---

  /// Toggle the `.active` class on comment cards to match `activeCommentId`,
  /// without triggering a full DOM rebuild.
  _updateActiveCommentClass() {
    const list = document.getElementById('commentsList');
    if (!list) return;
    const oldActive = list.querySelector('.comment-card.active');
    if (oldActive && parseInt(oldActive.dataset.commentId, 10) !== this.activeCommentId) {
      oldActive.classList.remove('active');
    }
    if (this.activeCommentId != null) {
      const newActive = list.querySelector('.comment-card[data-comment-id="' + this.activeCommentId + '"]');
      if (newActive) newActive.classList.add('active');
    }
  },

  /// Save the state of any active textarea inputs (reply forms) so they can
  /// be restored after an incremental DOM update.
  _saveActiveForms() {
    const forms = {};
    document.querySelectorAll('.reply-form-area textarea').forEach(ta => {
      const area = ta.closest('.reply-form-area');
      if (area && area.id && ta.value) {
        const commentId = area.id.replace('replyFormArea', '');
        forms['reply_' + commentId] = {
          value: ta.value,
          selectionStart: ta.selectionStart,
          selectionEnd: ta.selectionEnd,
          commentId: commentId,
        };
      }
    });
    return forms;
  },

  /// Restore previously saved form state after a DOM update.
  _restoreActiveForms(saved) {
    if (!saved || Object.keys(saved).length === 0) return;
    for (const [, data] of Object.entries(saved)) {
      if (!data.commentId) continue;
      const commentId = data.commentId;
      const area = document.getElementById('replyFormArea' + commentId);
      if (!area) continue;
      area.dataset.visible = 'true';
      area.innerHTML =
        '<div class="reply-form" onclick="event.stopPropagation()">' +
        '<textarea id="replyInput' + commentId + '" placeholder="输入回复... 以 @AGENT 开头继续向 AGENT 提问" rows="2">' + this.escape(data.value) + '</textarea>' +
        '<div class="reply-form-actions">' +
        '<button onclick="app.hideReplyForm(' + commentId + ')">取消</button>' +
        '<button class="primary" onclick="app.submitReply(' + commentId + ')">提交</button>' +
        '</div></div>';
      const ta = document.getElementById('replyInput' + commentId);
      if (ta && data.selectionStart != null) {
        ta.setSelectionRange(data.selectionStart, data.selectionEnd || data.selectionStart);
      }
    }
  },

  /// Render a reply with CSS-line-clamp truncation and an expand action.
  /// All replies use the same structure: short (CSS-clamped to 1 line) +
  /// full (hidden) + actions.  The expand button is hidden by default and
  /// revealed by `_revealExpandButtons` after render if the content overflows.
  /// `options.suffix` creates unique IDs when the same comment needs multiple
  /// expandable regions (e.g. user prompt + AI reply on an AI annotation).
  _renderExpandableReply(text, id, isAi, options = {}) {
    const suffix = options.suffix || '';
    const extraClass = options.className || '';
    const isMd = isAi && this.isMarkdown(text);
    const usedTool = isAi && !!(this.comments.find(c => c.id === id)?.ai_old_text);
    const aiStatus = options.aiStatus || '';
    const regenerateBtn = isAi && aiStatus !== 'applied' && this.authenticated
      ? '<button class="comment-expand-regenerate-btn" onclick="event.stopPropagation();app.regenerateAiComment(' + id + ', event)" title="重新生成"><svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="23 4 23 10 17 10"></polyline><path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"></path></svg></button>'
      : '';

    // AI markdown / non-tool replies use the modal viewer for full content;
    // plain text uses an inline toggle.  The primary action icon is placed
    // before the regenerate icon so both sit at the end of the single-line row.
    const usesModalViewer = isAi && (!usedTool || isMd);
    const expandAction = usesModalViewer
      ? '<button class="comment-expand-btn view-full-reply-btn" onclick="event.stopPropagation();app.showReplyModal(' + id + ', event)" title="查看完整回复">查看完整回复</button>'
      : '<button class="comment-expand-btn" id="commentExpandToggle' + id + suffix + '" onclick="event.stopPropagation();app.toggleExpandReply(' + id + ', \'" + suffix + "\')" title="展开">&#9660;</button>';

    // Full text is always present (hidden).  For the modal path it is
    // invisible but needed by showReplyModal; for the toggle path it is
    // swapped in by toggleExpandReply.
    const fullId = 'commentExpandFull' + id + suffix;
    const shortId = 'commentExpandShort' + id + suffix;

    const viewFullClass = usesModalViewer ? ' has-view-full' : '';
    return '<div class="comment-expand-reply' + viewFullClass + (extraClass ? ' ' + extraClass : '') + '">'
      + (usesModalViewer ? '' : '<div class="comment-expand-actions">' + expandAction + '</div>')
      + '<div class="comment-expand-short" id="' + shortId + '">' + this.escape(text) + '</div>'
      + '<div class="comment-expand-full" id="' + fullId + '" style="display:none">' + this.escape(text) + '</div>'
      + (usesModalViewer ? '<div class="view-full-reply-row">' + expandAction + '</div>' : '')
      + (regenerateBtn ? '<div class="comment-expand-actions regenerate-actions">' + regenerateBtn + '</div>' : '')
      + '</div>';
  },

  /// After rendering, hide expand/collapse buttons whose short text
  /// does not actually overflow the 1-line CSS clamp.  This avoids
  /// showing "展开" on replies that fit within 1 line.
  _revealExpandButtons() {
    const cards = document.querySelectorAll('.comment-expand-reply');
    for (const card of cards) {
      const short = card.querySelector('.comment-expand-short');
      const actions = card.querySelector('.comment-expand-actions');
      if (!short || !actions) continue;
      // If the clamped text overflows, show the actions; otherwise hide.
      const overflows = short.scrollHeight > short.clientHeight + 2;
      actions.style.display = overflows ? '' : 'none';
      // If not overflowing, also hide the full-text div
      if (!overflows) {
        const full = card.querySelector('.comment-expand-full');
        if (full) full.style.display = 'none';
      }
    }
  },

  /// Toggle between short and full text in an expandable reply.
  toggleExpandReply(id, suffix = '') {
    const short = document.getElementById('commentExpandShort' + id + suffix);
    const full = document.getElementById('commentExpandFull' + id + suffix);
    const btn = document.getElementById('commentExpandToggle' + id + suffix);
    if (!short || !full || !btn) return;
    if (full.style.display === 'none') {
      full.style.display = '';
      short.style.display = 'none';
      btn.innerHTML = '&#9650;';
      btn.title = '收起';
    } else {
      full.style.display = 'none';
      short.style.display = '';
      btn.innerHTML = '&#9660;';
      btn.title = '展开';
    }
  },

  /// Render a single comment card (with nested replies) as an HTML string.
  /// Extracted from the old `renderCard` closure for reuse in incremental updates.
  _renderCommentCard(c, childrenMap, lastId) {
    const isLast = lastId === c.id;
    const isAi = c.is_ai;
    const aiStatus = c.ai_status || '';
    let bodyHtml = '';
    let actionHtml = '';

    if (isAi) {
      bodyHtml = this._renderExpandableReply(c.comment, c.id, false, { suffix: 'Prompt', className: 'ai-prompt-expand' });
      if (aiStatus === 'pending') {
        bodyHtml += '<div class="ai-comment-status"><span class="ai-spinner"></span>处理中...</div>';
      } else if (aiStatus === 'completed' || aiStatus === 'applied' || aiStatus === 'rejected') {
        if (c.ai_reply) {
          bodyHtml += this._renderExpandableReply(c.ai_reply, c.id, true, { aiStatus });
        }
        if (aiStatus === 'completed' && c.ai_old_text && c.ai_new_text) {
          actionHtml = '<div class="ai-comment-actions">' +
            '<button class="btn-primary" onclick="event.stopPropagation();app.applyAiComment(' + c.id + ')">接受</button>' +
            '<button class="btn-secondary" onclick="event.stopPropagation();app.rejectAiComment(' + c.id + ')">取消</button>' +
            '</div>';
        } else if (aiStatus === 'applied') {
          actionHtml = '<div class="ai-comment-status-tag applied">已接受</div>';
        } else if (aiStatus === 'rejected') {
          actionHtml = '<div class="ai-comment-status-tag rejected">已拒绝</div>';
        }
      } else if (aiStatus === 'failed') {
        bodyHtml += '<div class="ai-comment-status error">AI 处理失败</div>';
      }
    } else {
      // Regular comment: use expandable structure so CSS clamp + reveal
      // handles truncation for any text length.
      bodyHtml = this._renderExpandableReply(c.comment, c.id, false);
    }

    const replyBtn = this.authenticated
      ? '<button class="reply-btn" onclick="event.stopPropagation();app.showReplyForm(' + c.id + ')" title="回复">回复</button>'
      : '';

    let html = '<div class="comment-card ' + (isAi ? 'ai-comment ' : '') + (this.activeCommentId === c.id ? 'active ' : '') + (isLast ? ' anim-enter' : '') + '" data-comment-id="' + c.id + '" onclick="app.onCommentCardClick(' + c.id + ')" onmouseenter="app.onCommentCardHover(' + c.id + ')" onmouseleave="app.onCommentCardLeave()">' +
      (c.parent_id ? '' : (this.isImageQuote(c.quote) ? '<div class="comment-card-image"><img src="' + this.escape(c.quote) + '" loading="lazy"></div>' : '<div class="comment-card-quote">' + this.escape(c.quote) + '</div>')) +
      bodyHtml + actionHtml +
      '<div class="comment-card-meta"><span>' + this.formatShortDate(c.created_at) + '</span><div class="comment-card-meta-actions">' + replyBtn + (this.authenticated ? '<button class="delete-btn" onclick="event.stopPropagation();app.deleteComment(' + c.id + ')" title="删除">删除</button>' : '') + '</div></div>' +
      '<div class="reply-form-area" id="replyFormArea' + c.id + '"></div>';

    const children = childrenMap.get(c.id);
    if (children) {
      html += '<div class="comment-replies">';
      for (const child of children) {
        html += this._renderCommentCard(child, childrenMap, lastId);
      }
      html += '</div>';
    }
    html += '</div>';
    return html;
  },

  /// Compute a fingerprint string for a root comment and its children,
  /// used to decide whether an incremental update is needed.
  _computeGroupFingerprint(rootComment, childrenMap) {
    const parts = [rootComment.id + ':' + rootComment.ai_status + ':' + (rootComment.ai_status === 'pending' ? 'P' : (rootComment.ai_reply ? rootComment.ai_reply.length : 0))];
    const children = childrenMap.get(rootComment.id) || [];
    for (const ch of children) {
      parts.push(ch.id + ':' + ch.ai_status + ':' + (ch.ai_status === 'pending' ? 'P' : (ch.ai_reply ? ch.ai_reply.length : 0)));
    }
    return parts.join('|') + ':' + !!this.authenticated;
  },

  /// Incrementally update the comment list: only rebuild cards whose
  /// fingerprint changed, keeping unchanged cards (and their interactive
  /// state) in place.  This prevents the 3-second polling refresh from
  /// causing visual flicker or destroying in-progress form inputs.
  _incrementalUpdateComments(list, roots, childrenMap, newGroupFps, lastId) {
    const rootIds = new Set(roots.map(c => c.id));

    // Map existing root cards by their comment ID
    const existingRoots = new Map();
    for (const el of list.querySelectorAll(':scope > .comment-card')) {
      const id = parseInt(el.dataset.commentId, 10);
      if (!isNaN(id)) existingRoots.set(id, el);
    }

    // Remove cards for deleted comments
    for (const [id, el] of existingRoots) {
      if (!rootIds.has(id)) {
        el.remove();
      }
    }

    // Update or insert cards in order
    for (let i = 0; i < roots.length; i++) {
      const c = roots[i];
      const newFp = newGroupFps.get(c.id);
      const oldFp = this._commentFingerprints ? this._commentFingerprints.get(c.id) : undefined;
      const existingEl = existingRoots.get(c.id);

      if (existingEl && oldFp === newFp) {
        // Unchanged — just fix the active class
        if (this.activeCommentId === c.id) {
          existingEl.classList.add('active');
        } else {
          existingEl.classList.remove('active');
        }
        continue;
      }

      // Changed or new — rebuild this group
      const newHtml = this._renderCommentCard(c, childrenMap, lastId);
      const temp = document.createElement('div');
      temp.innerHTML = newHtml;
      const newEl = temp.firstElementChild;

      if (existingEl) {
        existingEl.replaceWith(newEl);
      } else {
        // Insert at correct position (before the next existing sibling)
        let insertBefore = null;
        for (let j = i + 1; j < roots.length; j++) {
          const nextExisting = existingRoots.get(roots[j].id);
          if (nextExisting && nextExisting.parentNode === list) {
            insertBefore = nextExisting;
            break;
          }
        }
        if (insertBefore) {
          list.insertBefore(newEl, insertBefore);
        } else {
          list.appendChild(newEl);
        }
      }
    }

    if (lastId !== null) {
      setTimeout(() => { this.lastAddedCommentId = null; }, 500);
    }
  },
};
