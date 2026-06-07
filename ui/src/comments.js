// Auto-extracted from the original monolithic app.js.
// Each module exports a slice of the global `app` object; main.js merges them.
export const comments = {
  buildHeadingPrompt() {
    return '第一步，深度理解全文，然后在<thinking>中寻找一个与论文做类比最贴切的生活化场景（比如：后厨之协作、吃火锅之节奏、行程之策略），要输出理由和思路。第二步，你要扮演<thinking>场景中的主角，你在该领域已有三十年的经验。第三步，你用三十年的心法做沉浸式类比，套用文中的方法和公式，以痛点和解决方法作为主线逻辑。要求使用针对画面与感官的夸张+胡扯手法，结构化输出但不要标题，段落间使用`<hr>`。不要输出额外说明或元信息。以「老弟，你坐下，听我讲。」开始续写';
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
    // If a reply bubble is already open the user is reading a specific reply.
    // Polling refreshes must leave it alone — the auto-open logic below only
    // targets pending/empty replies that the user would never open manually.
    const bubbleOpen = this._openReplyBubbleId != null;
    const hadBubble = !bubbleOpen && !!document.getElementById('replyBubble');
    if (hadBubble) this.hideReplyBubble();

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
    // (old bubble may still be in DOM during shrink animation, so check hadBubble not DOM)
    if (hadBubble) {
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
    const fp = this.comments.map(c => {
      // During pending, ai_reply grows on backend but isn't rendered (only
      // a spinner is shown), so ignore ai_reply length changes to avoid
      // re-rendering the entire list every 3 seconds during polling.
      const aiReplyLen = c.ai_status === 'pending' ? 'P' : (c.ai_reply ? c.ai_reply.length : 0);
      return c.id + ':' + c.ai_status + ':' + aiReplyLen;
    }).join(',') + '|auth:' + !!this.authenticated + '|active:' + (this.activeCommentId || '');
    if (fp === this._commentsFingerprint && this.comments.length > 0) return;
    this._commentsFingerprint = fp;

    if (countEl) countEl.textContent = this.comments.length;
    if (toggleCount) toggleCount.textContent = this.comments.length;
    if (this.comments.length === 0) {
      list.innerHTML = '<div class="empty-comments">暂无批注<br>选中正文添加批注</div>';
    } else {
      const lastId = this.lastAddedCommentId;
      const childrenMap = new Map();
      for (const c of this.comments) {
        if (c.parent_id) {
          if (!childrenMap.has(c.parent_id)) childrenMap.set(c.parent_id, []);
          childrenMap.get(c.parent_id).push(c);
        }
      }
      const renderCard = (c) => {
        const isLast = lastId === c.id;
        const isAi = c.is_ai;
        const aiStatus = c.ai_status || '';
        let bodyHtml = '';
        let actionHtml = '';

        if (isAi) {
          bodyHtml = `<div class="comment-card-body ai-comment-body">${this.escape(c.comment)}</div>`;
          if (aiStatus === 'pending') {
            bodyHtml += `<div class="ai-comment-status"><span class="ai-spinner"></span>处理中...</div>`;
          } else if (aiStatus === 'completed' || aiStatus === 'applied' || aiStatus === 'rejected') {
            if (c.ai_reply) {
              const MAX_LEN = 150;
              const usedTool = !!(c.ai_old_text && c.ai_new_text);
              const needsTruncate = c.ai_reply.length > MAX_LEN;
              if (!usedTool) {
                // No tool used: always show "view full reply" button
                const short = this.escape(this.truncateText(c.ai_reply, MAX_LEN));
                bodyHtml += `<div class="ai-comment-reply"><div class="ai-reply-short">${short}</div><div class="ai-reply-actions"><button class="ai-reply-expand-btn" onclick="event.stopPropagation();app.showReplyModal(${c.id}, event)">查看完整回复</button><button class="ai-reply-regenerate-btn" onclick="event.stopPropagation();app.regenerateAiComment(${c.id}, event)">↻</button></div></div>`;
              } else if (needsTruncate) {
                const isMd = this.isMarkdown(c.ai_reply);
                const short = this.escape(this.truncateText(c.ai_reply, MAX_LEN));
                if (isMd) {
                  bodyHtml += `<div class="ai-comment-reply"><div class="ai-reply-short">${short}</div><div class="ai-reply-actions"><button class="ai-reply-expand-btn" onclick="event.stopPropagation();app.showReplyModal(${c.id}, event)">查看完整回复</button><button class="ai-reply-regenerate-btn" onclick="event.stopPropagation();app.regenerateAiComment(${c.id}, event)">↻</button></div></div>`;
                } else {
                  bodyHtml += `<div class="ai-comment-reply"><div class="ai-reply-short" id="aiReplyShort${c.id}">${short}</div><div class="ai-reply-full" id="aiReplyFull${c.id}" style="display:none">${this.escape(c.ai_reply)}</div><div class="ai-reply-actions"><button class="ai-reply-expand-btn" id="aiReplyToggle${c.id}" onclick="event.stopPropagation();app.toggleAiReply(${c.id})">展开</button><button class="ai-reply-regenerate-btn" onclick="event.stopPropagation();app.regenerateAiComment(${c.id}, event)">↻</button></div></div>`;
                }
              } else {
                bodyHtml += `<div class="ai-comment-reply">${this.escape(c.ai_reply)}<div class="ai-reply-actions"><button class="ai-reply-regenerate-btn" onclick="event.stopPropagation();app.regenerateAiComment(${c.id}, event)">↻</button></div></div>`;
              }
            }
            if (aiStatus === 'completed' && c.ai_old_text && c.ai_new_text) {
              actionHtml = `<div class="ai-comment-actions">
                <button class="btn-primary" onclick="event.stopPropagation();app.applyAiComment(${c.id})">接受</button>
                <button class="btn-secondary" onclick="event.stopPropagation();app.rejectAiComment(${c.id})">取消</button>
              </div>`;
            } else if (aiStatus === 'applied') {
              actionHtml = `<div class="ai-comment-status-tag applied">已接受</div>`;
            } else if (aiStatus === 'rejected') {
              actionHtml = `<div class="ai-comment-status-tag rejected">已拒绝</div>`;
            }
          } else if (aiStatus === 'failed') {
            bodyHtml += `<div class="ai-comment-status error">AI 处理失败</div>`;
          }
        } else {
          bodyHtml = `<div class="comment-card-body">${this.escape(c.comment)}</div>`;
        }

        const replyBtn = this.authenticated
          ? `<button class="reply-btn" onclick="event.stopPropagation();app.showReplyForm(${c.id})">回复</button>`
          : '';

        let html = `
        <div class="comment-card ${isAi ? 'ai-comment' : ''} ${this.activeCommentId === c.id ? 'active' : ''}${isLast ? ' anim-enter' : ''}" data-comment-id="${c.id}" onclick="app.onCommentCardClick(${c.id})"
          onmouseenter="app.onCommentCardHover(${c.id})" onmouseleave="app.onCommentCardLeave()"
        >
          ${c.parent_id ? '' : (this.isImageQuote(c.quote) ? `<div class="comment-card-image"><img src="${this.escape(c.quote)}" loading="lazy"></div>` : `<div class="comment-card-quote">${this.escape(c.quote)}</div>`)}
          ${bodyHtml}
          ${actionHtml}
          <div class="comment-card-meta">
            <span>${this.formatShortDate(c.created_at)}</span>
            <div class="comment-card-meta-actions">
              ${replyBtn}
              ${this.authenticated ? `<button class="delete-btn" onclick="event.stopPropagation();app.deleteComment(${c.id})">删除</button>` : ''}
            </div>
          </div>
          <div class="reply-form-area" id="replyFormArea${c.id}"></div>
        `;

        const children = childrenMap.get(c.id);
        if (children) {
          html += '<div class="comment-replies">';
          for (const child of children) {
            html += renderCard(child);
          }
          html += '</div>';
        }
        html += '</div>';
        return html;
      };

      const roots = this.comments.filter(c => !c.parent_id);
      list.innerHTML = roots.map(c => renderCard(c)).join('');
      if (lastId !== null) {
        setTimeout(() => { this.lastAddedCommentId = null; }, 500);
      }
    }
    // Clear any pending form
    if (formArea) formArea.innerHTML = '';
    // Update panel/toggle visibility based on current state
    const panel = document.getElementById('insightCommentsPanel');
    const toggle = document.getElementById('commentsToggleBtn');
    if (this.comments.length === 0) {
      // No annotations: never surface the toggle button.
      if (panel) panel.classList.remove('open');
      if (toggle) toggle.classList.remove('show');
    } else if (panel) {
      if (this.commentsPanelOpen) {
        panel.classList.add('open');
        if (toggle) toggle.classList.remove('show');
      } else {
        // Show toggle button when panel is collapsed
        panel.classList.remove('open');
        if (toggle) toggle.classList.add('show');
      }
    }
  },

  showReplyModal(id, event) {
    const comment = this.comments.find(c => c.id === id);
    if (!comment || !comment.ai_reply) return;
    // Track which reply is open so polling refreshes don't destroy it
    this._openReplyBubbleId = id;
    this.hideReplyBubble();

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
    `;
    document.body.appendChild(bubble);

    const inner = bubble.querySelector('.reply-bubble-inner');
    inner.innerHTML = this.renderMarkdown(this.escapeHtml(comment.ai_reply));

    // Show comment text in drag handle (visible in immersive mode)
    const handleText = bubble.querySelector('.reply-bubble-handle-text');
    if (handleText && comment.comment) {
      handleText.textContent = comment.comment;
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

    this._hideReplyBubbleHandler = (e) => {
      if (!bubble.contains(e.target) && e.target !== btn) {
        e.stopPropagation();
        this.hideReplyBubble();
      }
    };
    const isRestore = !event || !event.target;
    requestAnimationFrame(() => {
      document.addEventListener('click', this._hideReplyBubbleHandler);
      // Auto-enter immersive: mobile, long content, or restoring from refresh
      const isMobile = !this.isDesktopViewport();
      if (isRestore || isMobile || (comment.ai_reply && comment.ai_reply.length > 400)) {
        this.enterImmersiveMode();
      }
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
    bubble.style.maxWidth = '';
    const vw = window.innerWidth;
    const isMobile = vw <= 640;
    // Match CSS: .reply-bubble.immersive { width: min(780px, 90vw) }
    const targetWidth = isMobile ? vw * 0.95 : Math.min(780, vw * 0.9);
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

  hideReplyBubble() {
    // Allow loadComments to auto-open a reply again after the bubble is closed
    this._openReplyBubbleId = null;
    const bubble = document.getElementById('replyBubble');

    // Always remove click handler first to prevent double-trigger during animation
    if (this._hideReplyBubbleHandler) {
      document.removeEventListener('click', this._hideReplyBubbleHandler);
      delete this._hideReplyBubbleHandler;
    }

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
        if (b) b.remove();
        if (this._bubbleDragCleanup) {
          this._bubbleDragCleanup();
          delete this._bubbleDragCleanup;
        }
        delete this._replyBubbleOriginalRect;
        delete this._replyBubbleBtnRect;
        this._updateThemeColorForBubble(false);
      };
      if (btnRect && this.isDesktopViewport()) {
        setTimeout(cleanup, 300);
      } else {
        // Mobile: wait for exitImmersiveMode transition to finish
        setTimeout(cleanup, 300);
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
    delete this._replyBubbleOriginalRect;
    delete this._replyBubbleBtnRect;
    this._updateThemeColorForBubble(false);
  },

  hideReplyModal() {
    this.hideReplyBubble();
  },

  toggleAiReply(id) {
    const short = document.getElementById('aiReplyShort' + id);
    const full = document.getElementById('aiReplyFull' + id);
    const btn = document.getElementById('aiReplyToggle' + id);
    if (!short || !full || !btn) return;
    if (full.style.display === 'none') {
      full.style.display = '';
      short.style.display = 'none';
      btn.textContent = '收起';
    } else {
      full.style.display = 'none';
      short.style.display = '';
      btn.textContent = '展开';
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
    const editor = document.getElementById('mobileCommentEditor');
    const input = document.getElementById('mobileCommentEditorInput');
    const quote = document.getElementById('mobileCommentEditorQuote');
    const title = editor ? editor.querySelector('.mobile-comment-editor-title') : null;
    if (!editor || !input || !quote) return;

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

    editor.classList.add('show');
    // Save scroll position before locking body (iOS jumps to top)
    this._editorScrollY = window.scrollY;
    document.body.style.overflow = 'hidden';

    // Focus and position cursor at end
    setTimeout(() => {
      input.focus();
      input.setSelectionRange(input.value.length, input.value.length);
      // Adjust for keyboard using visualViewport
      this._adjustEditorForKeyboard();
    }, 100);
  },

  hideMobileCommentEditor() {
    const editor = document.getElementById('mobileCommentEditor');
    if (editor) {
      editor.classList.remove('show');
      editor.style.transform = '';
    }
    document.body.style.overflow = '';
    // Restore scroll position after unlocking body
    if (this._editorScrollY != null) {
      window.scrollTo(0, this._editorScrollY);
      this._editorScrollY = null;
    }
    document.dispatchEvent(new Event('app:hideMobileEditor'));
    this._removeKeyboardHandler();
    this.hideSelectionTooltip();
    this.cancelCommentForm();
  },

  _adjustEditorForKeyboard() {
    if (!window.visualViewport) return;
    // Remove previous handlers first (safe even if never added)
    if (this._kbHandler) {
      window.visualViewport.removeEventListener('resize', this._kbHandler);
      window.visualViewport.removeEventListener('scroll', this._kbHandler);
    }
    this._kbHandler = () => {
      const vh = window.visualViewport.height;
      const keyboardH = window.innerHeight - vh;
      const editor = document.getElementById('mobileCommentEditor');
      if (!editor) return;
      if (keyboardH > 50) {
        editor.style.transform = 'translateY(-' + keyboardH + 'px)';
      } else {
        editor.style.transform = '';
      }
    };
    window.visualViewport.addEventListener('resize', this._kbHandler);
    window.visualViewport.addEventListener('scroll', this._kbHandler);
  },

  _removeKeyboardHandler() {
    if (this._kbHandler) {
      window.visualViewport?.removeEventListener('resize', this._kbHandler);
      window.visualViewport?.removeEventListener('scroll', this._kbHandler);
      this._kbHandler = null;
    }
  },

  async submitMobileComment() {
    const input = document.getElementById('mobileCommentEditorInput');
    const text = input ? input.value.trim() : '';
    if (!text) return;
    if (!this.pendingSelection && !this.pendingImage) return;
    if (!this.insightPageId) return;

    this.haptic('success');

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
    this.haptic('success');
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
        this.pendingSelection = null;
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
      this.pendingSelection = null;
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
      this.pendingImage = null;
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
    if (this._aiPollTimer) return;
    this._aiPollTimer = setInterval(async () => {
      if (!this.insightPageId) {
        clearInterval(this._aiPollTimer);
        this._aiPollTimer = null;
        return;
      }
      const loaded = await this.loadComments(this.insightPageId);
      if (!loaded) return;
      const hasPending = this.comments.some(c => c.is_ai && c.ai_status === 'pending');
      if (!hasPending) {
        clearInterval(this._aiPollTimer);
        this._aiPollTimer = null;
      }
    }, 3000);
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
    this.activeCommentId = id;
    this.updateHighlightActive();
    this.renderComments();
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
};
