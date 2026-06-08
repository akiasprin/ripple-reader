// Auto-extracted from the original monolithic app.js.
// Each module exports a slice of the global `app` object; main.js merges them.
export const insight = {
  async loadInsightPage(id) {
    this.abortRequest('insight');
    const ctrl = new AbortController();
    this.abortControllers.set('insight', ctrl);
    const wasAlreadyOnThisPage = this.insightPageData && this.insightPageData.id === id;
    if (!wasAlreadyOnThisPage) {
      this.stopInsightStream();
    }
    try {
      const t0 = performance.now();
      const res = await fetch('/api/papers/' + encodeURIComponent(this.paperSource(id)) + '/' + encodeURIComponent(id), { signal: ctrl.signal });
      if (!res.ok) throw new Error('not found');
      this.insightPageData = await res.json();
      if (this.insightPageData && this.insightPageData.source_type) {
        this.insightPageSource = this.insightPageData.source_type;
        this.replaceURL();
      }
      this.abortControllers.delete('insight');
      console.log('[perf] fetch+json:', (performance.now() - t0).toFixed(0), 'ms');
      const p = this.insightPageData;
      const isBusy = p.insight === '__ANALYZING__' || p.insight_review === '__REVIEWING__';
      if (isBusy && !this.pollingIds.has(id)) {
        this.addPollingId(id);
      }
      const t1 = performance.now();
      this.renderInsightPage();
      console.log('[perf] renderInsightPage:', (performance.now() - t1).toFixed(0), 'ms');
      if (!wasAlreadyOnThisPage) {
        window.scrollTo(0, 0);
        requestAnimationFrame(() => window.scrollTo(0, 0));
      }
      this.loadInsightNeighbors(id);
    } catch (e) {
      if (e.name === 'AbortError') return;
      this.abortControllers.delete('insight');
      document.getElementById('insightPage').innerHTML = '<div class="empty">加载失败，请返回重试</div>';
    }
  },

  async showInsightPage(id, source, skipPush) {
    this.haptic('medium');
    // Reset pseudocode.js caption counter so algorithm numbering restarts on each paper
    if (typeof pseudocode !== 'undefined' && pseudocode.Renderer) {
      pseudocode.Renderer.captionCount = 0;
    }
    this.insightPageId = id;
    this.insightPageSource = source || this.paperSource(id);
    this._updateProgressHighlight();
    // Clear the previous paper's comments first so the toggle visibility below is judged
    // against the new (empty) state, not the prior paper's count.
    this.comments = [];
    this._commentsFingerprint = '';
    this._highlightsFingerprint = '';
    this.activeCommentId = null;
    this.hideCommentsPanel(true);
    // Reset editing state to prevent leaking state from previous paper
    this.insightPageEditing = false;
    if (!skipPush) this.pushURL();
    this.enterInsightView();
    // Adjust layout immediately after DOM changes
    this.adjustLayoutForComments();
    await this.loadInsightPage(id);
    // Adjust layout again after page data loads, but only if still on this paper
    if (this.insightPageId === id) {
      this.adjustLayoutForComments();
    }
  },

  enterInsightView() {
    // Hide list UI elements when switching to insight page view
    const alreadyOpen = document.getElementById('insightPage').classList.contains('open');
    // Save list scroll position BEFORE hiding elements (hiding collapses page height)
    if (!alreadyOpen) {
      this._listScrollY = window.scrollY || document.documentElement.scrollTop;
    }
    document.getElementById('filterBar').style.display = 'none';
    document.getElementById('stats').style.display = 'none';
    document.getElementById('list').style.display = 'none';
    document.getElementById('pagination').style.display = 'none';
    document.getElementById('tagsPage').classList.remove('open');
    this.tagsPageOpen = false;
    document.getElementById('datesPage').classList.remove('open');
    this.datesPageOpen = false;
    document.getElementById('insightPage').classList.add('open');
    this._updateThemeColor(document.documentElement.getAttribute('data-theme') === 'dark', true);
    window.scrollTo(0, 0);
  },

  async clearInsight(id) {
    const headers = {};
    if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
    try {
      const res = await fetch('/api/papers/' + encodeURIComponent(this.paperSource(id)) + '/' + encodeURIComponent(id) + '/insight', {
        method: 'DELETE',
        headers,
      });
      if (!res.ok) {
        this.showToast('清空失败', 'error');
        return;
      }
      const updated = await res.json();
      this.insightPageData = this.normalizePaper(updated);
      this.insightPageEditing = false;
      this.renderInsightPage();
      this.showToast('已清空深度分析内容', 'success');
    } catch (e) {
      console.error('[clearInsight]', e);
      this.showToast('清空失败', 'error');
    }
  },

  async cancelInsight(id) {
    const headers = {};
    if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
    try {
      const res = await fetch('/api/papers/' + encodeURIComponent(this.paperSource(id)) + '/' + encodeURIComponent(id) + '/insight', {
        method: 'DELETE',
        headers,
      });
      if (!res.ok) {
        this.showToast('取消失败', 'error');
        return;
      }
      const updated = await res.json();
      if (this.insightStreamId === id) {
        this.stopInsightStream();
      }
      this.removePollingId(id);
      if (this.data && this.data.papers) {
        const idx = this.data.papers.findIndex(p => p.id === id);
        if (idx >= 0) {
          this.data.papers[idx] = this.normalizePaper(updated);
          this.renderList();
        }
      }
      if (this.insightPageData && this.insightPageData.id === id) {
        this.insightPageData = this.normalizePaper(updated);
        this.insightPageEditing = false;
        this.renderInsightPage();
      }
      this.showToast('已取消分析', 'success');
    } catch (e) {
      console.error('[cancelInsight]', e);
      this.showToast('取消失败', 'error');
    }
  },

  editInsight() {
    this.insightPageEditing = true;
    this.pushURL();
    this.renderInsightPage();
    const ta = document.getElementById('insightEditArea');
    if (ta) {
      ta.scrollTop = 0;
      ta.setSelectionRange(0, 0);
    }
  },

  cancelInsightPageEdit() {
    this.insightPageEditing = false;
    this.pushURL();
    this.renderInsightPage();
  },

  async loadInsightNeighbors(id) {
    try {
      const res = await fetch('/api/papers/' + encodeURIComponent(this.paperSource(id)) + '/' + encodeURIComponent(id) + '/insight-neighbors');
      if (!res.ok) return;
      const data = await res.json();
      if (!this.isCurrentInsight(id)) return;
      this.insightNeighbors = data;
    } catch (e) {
      this.insightNeighbors = { prev: null, next: null };
    }
  },

  async loadRelatedPapers(id) {
    try {
      const res = await fetch('/api/papers/' + encodeURIComponent(this.paperSource(id)) + '/' + encodeURIComponent(id) + '/related');
      if (!res.ok) return;
      const papers = await res.json();
      if (!this.isCurrentInsight(id)) return;
      const el = document.getElementById('relatedPapers');
      if (!el || !papers.length) {
        if (el) el.style.display = 'none';
        return;
      }
      el.style.display = '';
      el.innerHTML = `<h4>相关论文</h4>` + papers.map(p => `
        <div class="related-item" onclick="app.showInsightPage('${this.escape(p.id)}', '${this.escape(p.source_type || 'arxiv')}')">
          <span class="rel-score">${(p.similarity * 100).toFixed(0)}%</span>
          <span class="rel-title">${this.escape(p.title)}</span>
          <span class="rel-score" style="background:#4a7c59">${p.score.toFixed(1)}</span>
        </div>
      `).join('');
    } catch (e) {}
  },

  async loadPaperTokens(id) {
    try {
      const res = await fetch('/api/papers/' + encodeURIComponent(this.paperSource(id)) + '/' + encodeURIComponent(id) + '/tokens');
      if (!res.ok) return;
      const data = await res.json();
      if (!this.isCurrentInsight(id)) return;
      const el = document.getElementById('insightTokenCount');
      if (el) {
        const parts = [];
        const p = this.insightPageData;
        if (p && p.insight) parts.push(p.insight.length + ' 字');
        if (data.insight_tokens) parts.push(data.insight_tokens + ' tokens');
        el.textContent = parts.join(' · ');
      }
    } catch (e) {}
  },

  async searchByTag(tag) {
    this.searchQuery = tag;
    this.page = 1;
    this.insightPageId = null;
    this.pushURL();
    this.exitInsightView();
    document.getElementById('searchInput').value = tag;
    await this.load();
    window.scrollTo(0, 0);
  },

  async saveInsight(id) {
    const ta = document.getElementById('insightEditArea');
    if (!ta) return;
    const updates = { insight: ta.value };
    try {
      const headers = { 'Content-Type': 'application/json' };
      if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
      const res = await fetch('/api/papers/' + encodeURIComponent(this.paperSource(id)) + '/' + encodeURIComponent(id), {
        method: 'PUT',
        headers,
        body: JSON.stringify(updates),
      });
      if (!res.ok) {
        let reason = '保存失败';
        try {
          const err = await res.json();
          if (err && err.reason) reason = err.reason;
        } catch (_) {}
        throw new Error(reason);
      }
      const updated = await res.json();
      this.insightPageData = updated;
      this.insightPageEditing = false;
      this.pushURL();
      this.renderInsightPage();
      this.showToast('已保存', 'success');
    } catch (e) {
      console.error('[saveInsight]', e);
      this.showToast(e.message || '保存失败', 'error');
    }
  },

  regenerateInsight(id, event) {
    this.toggleInsight(id, event);
  },

  hideInsightPage() {
    this.haptic('light');
    this.insightPageId = null;
    this.insightPageData = null;
    this.insightPageEditing = false;
    this.comments = [];
    this.activeCommentId = null;
    this.stopInsightStream();
    this.pushURL();
    this.exitInsightView();
    this.hideCommentsPanel();
    this.renderList();
    // Restore scroll after renderList — use rAF to ensure layout is complete
    if (!this.isDesktopViewport() && this._listScrollY != null) {
      const savedY = this._listScrollY;
      this._listScrollY = null;
      requestAnimationFrame(() => {
        requestAnimationFrame(() => window.scrollTo(0, savedY));
      });
    }
  },

  exitInsightView() {
    // Restore list UI and clean up overlay pages when leaving insight/tags/dates view
    this.commentsPanelOpen = false;
    // The comments toggle is a fixed element outside insightPage, so hide it explicitly.
    const commentsToggle = document.getElementById('commentsToggleBtn');
    if (commentsToggle) commentsToggle.classList.remove('show');
    document.getElementById('insightPage').classList.remove('open');
    document.getElementById('insightPage').innerHTML = '';
    document.getElementById('tagsPage').classList.remove('open');
    document.getElementById('tagsPage').innerHTML = '';
    this.tagsPageOpen = false;
    document.getElementById('datesPage').classList.remove('open');
    document.getElementById('datesPage').innerHTML = '';
    this.datesPageOpen = false;
    this.hideOrphanPanel();
    document.getElementById('filterBar').style.display = '';
    document.getElementById('stats').style.display = '';
    document.getElementById('list').style.display = '';
    document.getElementById('pagination').style.display = '';
    document.getElementById('navHome').classList.add('active');
    document.getElementById('navTags').classList.remove('active');
    document.getElementById('navDates').classList.remove('active');
    document.title = 'Ripple';
    // Scroll position restore is deferred to hideInsightPage (after renderList)
    this._removeTOC();
    this._insightPageRendering = null;
    this._updateThemeColor(document.documentElement.getAttribute('data-theme') === 'dark', false);
    this.adjustLayoutForComments();
  },

  async _fetchInsightHtml(p) {
    const id = p.id;
    this._insightPageRendering = id;

    const body = document.getElementById('insightPageBody');
    const isDimmed = body && body.classList.contains('insight-dimmed');
    let loadingTimer = null;
    // Always start a 3s delayed loading timer.
    // If old content exists: dimmed state is visible, show spinner after 3s.
    // If no old content: loading state hidden, show spinner after 3s.
    loadingTimer = setTimeout(() => {
      if (this._insightPageRendering !== id) return;
      const el = document.getElementById('insightPageBody');
      if (!el) return;
      if (isDimmed) {
        el.classList.remove('insight-dimmed');
        this._updateThemeColor(document.documentElement.getAttribute('data-theme') === 'dark', false);
        el.innerHTML = '<div class="insight-loading-state"><div style="display:inline-block;width:24px;height:24px;border:3px solid var(--border);border-top-color:var(--accent);border-radius:50%;animation:spin 1s linear infinite;margin-bottom:12px;"></div><br>加载中...</div>';
      } else {
        const ls = el.querySelector('.insight-loading-state');
        if (ls) ls.style.display = '';
      }
    }, 3000);

    const cleanup = () => {
      if (loadingTimer) clearTimeout(loadingTimer);
      const b = document.getElementById('insightPageBody');
      if (b) {
        b.classList.remove('insight-dimmed');
        this._updateThemeColor(document.documentElement.getAttribute('data-theme') === 'dark', false);
      }
    };

    try {
      const t0 = performance.now();
      const res = await fetch('/api/papers/' + encodeURIComponent(this.paperSource(id)) + '/' + encodeURIComponent(id) + '/insight-html');
      if (!res.ok) { cleanup(); this._insightPageRendering = null; return; }
      const data = await res.json();
      console.log('[perf] fetchInsightHtml (' + id + '):', (performance.now() - t0).toFixed(0), 'ms, html:', data.html.length, 'chars, toc:', data.toc ? data.toc.length : 0, 'items');
      if (this.insightPageData?.id !== id) { cleanup(); this._insightPageRendering = null; return; }
      // Build review content
      let reviewHtml = '';
      if (p.insight_review && p.insight_review !== '__REVIEWING__') {
        const reviewDate = p.insight_reviewed_at ? this.formatDate(p.insight_reviewed_at) : '';
        reviewHtml = '<div class="insight-review-section"><div class="insight-review-header">Review 审核意见<span class="insight-review-status">已审核</span>' + (reviewDate ? '<span class="insight-review-date">' + reviewDate + '</span>' : '') + '</div><div class="insight-review-body">' + this.renderMarkdown(this.escapeHtml(p.insight_review), true, { deferKatex: true, deferPrettier: true }) + '</div></div>';
      }
      const renderBody = document.getElementById('insightPageBody');
      if (renderBody) {
        if (loadingTimer) clearTimeout(loadingTimer);
        renderBody.classList.remove('insight-dimmed');
        this._updateThemeColor(document.documentElement.getAttribute('data-theme') === 'dark', false);
        let insightHtml = this.renderPseudocode(data.html);
        // Strip <pre><code> wrappers that may enclose pseudocode blocks (server-side markdown already parsed)
        insightHtml = insightHtml.replace(/<pre><code(?:[^>]*)>([\s\S]*?<div class="ps-root">[\s\S]*?<\/div>[\s\S]*?)<\/code><\/pre>/g, '$1');
        renderBody.innerHTML = insightHtml + reviewHtml;
        this._buildTOCFromData(data.toc);
        this.renderDeferredMath(renderBody, () => this.renderHighlights());
        this.renderDeferredPrettier();
        this.loadProgressiveImages(renderBody);
      }
      this._insightPageRendering = null;
      if (typeof requestIdleCallback !== 'undefined') {
        requestIdleCallback(() => this.renderDeferredMetaInfo(), { timeout: 3000 });
      } else {
        setTimeout(() => this.renderDeferredMetaInfo(), 0);
      }
    } catch (e) {
      cleanup();
      console.error('[fetchInsightHtml]', e);
      this._insightPageRendering = null;
    }
  },

  renderInsightPage() {
    const p = this.insightPageData;
    if (!p) return;
    this._removeTOC();
    // If we already rendered the shell and are waiting for body, skip duplicate render
    if (this._insightPageRendering === p.id) {
      return;
    }
    const scoreClass = p.score >= 7 ? 'score-high' : p.score >= 4 ? 'score-mid' : 'score-low';
    document.title = p.title + ' · Ripple';
    const sourceUrl = p.source_type === 'arxiv'
      ? 'https://arxiv.org/abs/' + encodeURIComponent(p.external_id || p.id)
      : (p.source_url || '');
    const isMobile = !this.isDesktopViewport();
    const firstAuthor = p.authors[0] || '';
    const secondAuthor = p.authors[1] || '';
    const lastAuthor = p.authors.length > 2 ? p.authors[p.authors.length - 1] : '';
    let authorParts = [firstAuthor];
    if (secondAuthor && secondAuthor !== firstAuthor) authorParts.push(secondAuthor);
    if (lastAuthor && lastAuthor !== firstAuthor && lastAuthor !== secondAuthor) authorParts.push(lastAuthor);
    const authors = authorParts.join(', ');
    const isBusy = p.insight === '__ANALYZING__' || p.insight_review === '__REVIEWING__';
    const canEdit = this.authenticated && !isBusy;
    const iconMoonshot = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 6V3m0 0L9.5 1.5M12 3l2.5-1.5"/><circle cx="9.5" cy="1.5" r="0.8" fill="currentColor" stroke="none"/><circle cx="14.5" cy="1.5" r="0.8" fill="currentColor" stroke="none"/><rect x="3" y="6" width="18" height="15" rx="3"/><circle cx="9" cy="13" r="1.25" fill="currentColor" stroke="none"/><circle cx="15" cy="13" r="1.25" fill="currentColor" stroke="none"/><path d="M9.5 17c1.4 1 3.6 1 5 0"/></svg>';
    const iconEdit = '<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>';
    const iconReview = '<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><polyline points="16 11 18 13 22 9"/></svg>';
    const iconTrash = '<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>';
    const iconBackup = '<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="17 8 12 3 7 8"/><line x1="12" y1="3" x2="12" y2="15"/></svg>';
    const iconCheck = p.checked_at
      ? '<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="#4a7c59" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 11.08V12a10 10 0 1 1-5.93-9.14"/><polyline points="22 4 12 14.01 9 11.01"/></svg>'
      : '<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><path d="m9 12 2 2 4-4"/></svg>';

    const iconCancel = '<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><line x1="15" y1="9" x2="9" y2="15"/><line x1="9" y1="9" x2="15" y2="15"/></svg>';
    const isChecked = !!p.checked_at;
    const iconActions = `
      <span class="meta-icon-group">
        ${sourceUrl ? `<a href="${sourceUrl}" target="_blank" class="meta-icon" title="${p.source_type === 'arxiv' ? 'arXiv' : 'OpenReview'}"><svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/><polyline points="15 3 21 3 21 9"/><line x1="10" y1="14" x2="21" y2="3"/></svg></a>` : ''}
        <a href="/static/overlay.html?source=${encodeURIComponent(p.source_type || 'arxiv')}&id=${this.escape(p.id)}" target="_blank" class="meta-icon" title="bbox"><svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="2" ry="2"/><line x1="3" y1="9" x2="21" y2="9"/><line x1="9" y1="21" x2="9" y2="9"/></svg></a>
        ${p.insight === '__ANALYZING__' && this.authenticated ? `<span class="meta-icon" title="取消分析" onclick="app.cancelInsight('${this.escape(p.id)}')">${iconCancel}</span>` : ''}
        ${p.insight_review === '__REVIEWING__' && this.authenticated ? `<span class="meta-icon" title="取消审核" onclick="app.cancelReviewTask('${this.escape(p.id)}')">${iconCancel}</span>` : ''}
        ${!isChecked && canEdit ? `<span class="meta-icon" title="生成" onclick="app.regenerateInsight('${this.escape(p.id)}', event)">${iconMoonshot}</span>` : ''}
        ${!isChecked && canEdit ? `<a href="/insight/${encodeURIComponent(p.source_type || 'arxiv')}/${p.id.split('/').map(encodeURIComponent).join('/')}/edit" class="meta-icon" title="编辑" onclick="if (event.metaKey || event.ctrlKey) return true; event.preventDefault(); app.editInsight();">${iconEdit}</a>` : ''}
        ${!isChecked && canEdit ? `<span class="meta-icon" title="审查" onclick="app.reviewInsight('${this.escape(p.id)}', event)">${iconReview}</span>` : ''}
        ${!isChecked && canEdit ? `<span class="meta-icon" title="清空" onclick="app.clearInsight('${this.escape(p.id)}')">${iconTrash}</span>` : ''}
        ${!isChecked && canEdit ? `<span class="meta-icon" title="备份" onclick="app.showInsightBackups('${this.escape(p.id)}', event)">${iconBackup}</span>` : ''}
        ${canEdit || isChecked ? `<span class="meta-icon ${isChecked ? 'checked-active' : ''}" title="${isChecked ? '已检查' : '检查'}" onclick="app.toggleChecked('${this.escape(p.id)}')">${iconCheck}</span>` : ''}
      </span>
    `;

    let bodyContent = '';

    // Capture existing insight body for smooth transition (avoid loading spinner flash on fast loads)
    const oldInsightBody = document.getElementById('insightPageBody');
    const _oldBodyHtml = (oldInsightBody && !this._insightPageRendering &&
        oldInsightBody.innerHTML &&
        !oldInsightBody.textContent.includes('加载中') &&
        !oldInsightBody.textContent.includes('暂无'))
        ? oldInsightBody.innerHTML : '';

    if (this.insightPageEditing && canEdit) {
      bodyContent = `
        <div class="insight-editor">
          <textarea id="insightEditArea">${this.escape(p.insight)}</textarea>
          <div class="insight-editor-actions">
            <button class="btn-primary" onclick="app.saveInsight('${this.escape(p.id)}')">保存</button>
            <button class="btn-secondary" onclick="app.cancelInsightPageEdit()">取消</button>
          </div>
        </div>
      `;
    } else if (p.insight === '__ANALYZING__') {
      const cancelBtn = this.authenticated ? '<br><button class="btn-secondary" style="margin-top:16px;" onclick="app.cancelInsight(\'' + this.escape(p.id) + '\')">取消分析</button>' : '';
      bodyContent = '<div class="insight-loading-state" id="insightLoadingState"><div style="display:inline-block;width:24px;height:24px;border:3px solid var(--border);border-top-color:var(--accent);border-radius:50%;animation:spin 1s linear infinite;margin-bottom:12px;"></div><br>深度分析进行中，请稍候...' + cancelBtn + '</div><div id="insightStreamingContent" style="display:none;"></div>';
    } else if (p.insight) {
      bodyContent = '<div class="insight-loading-state' + (_oldBodyHtml ? ' insight-deferred' : '') + '" style="display:none;"><div style="display:inline-block;width:24px;height:24px;border:3px solid var(--border);border-top-color:var(--accent);border-radius:50%;animation:spin 1s linear infinite;margin-bottom:12px;"></div><br>加载中...</div>';
    } else {
      bodyContent = '<div class="insight-empty-state">暂无内容</div>';
    }

    const reviewDate = p.insight_reviewed_at ? this.formatDate(p.insight_reviewed_at) : '';
    const reviewStatus = p.insight_reviewed_at ? '<span class="insight-review-status">已审核</span>' : '';
    const reviewContent = p.insight_review === '__REVIEWING__' ? `
      <div class="insight-review-section">
        <div class="insight-review-header">
          Review 审核意见
          <span class="insight-review-status accent">审查中...</span>
        </div>
        <div class="insight-loading-state small">
          <div style="display:inline-block;width:20px;height:20px;border:3px solid var(--border);border-top-color:var(--accent);border-radius:50%;animation:spin 1s linear infinite;margin-bottom:8px;"></div>
          <br>审核后台运行中，请稍候...
          <br><button class="btn-secondary" style="margin-top:12px;" onclick="app.cancelReviewTask('${this.escape(p.id)}')">取消审核</button>
        </div>
      </div>
    ` : (p.insight_review ? `
      <div class="insight-review-section">
        <div class="insight-review-header">
          Review 审核意见
          ${reviewStatus}
          ${reviewDate ? `<span class="insight-review-date">${reviewDate}</span>` : ''}
        </div>
        <div class="insight-review-body">
          ${this.renderMarkdown(this.escapeHtml(p.insight_review), true, { deferKatex: true, deferPrettier: true })}
        </div>
      </div>
    ` : '');

    const html = `
      <div class="insight-page-header">
        <div class="insight-page-header-inner">
          <h2>${this.escape(p.title)}</h2>
          <button class="insight-page-close" onclick="app.hideInsightPage()" style="display:none;">&times;</button>
        </div>
        <div class="paper-meta">
          ${p.paper_type ? `<span class="type-badge">${this.escape(p.paper_type)}</span>` : ''}
          <div class="meta-left">
            <span>${this.escape(authors)}</span>
            ${iconActions}
          </div>
          <div class="meta-right">
            <span>${this.formatDate(p.published).slice(0, 10)}</span>
            <svg class="insight-meta-toggle" onclick="app.toggleInsightMetaInfo()" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"/></svg>
          </div>
        </div>
        ${this._renderInsightMetaInfo(p, scoreClass)}
      </div>
      <div class="insight-page-body" id="insightPageBody">
        ${bodyContent}
        ${reviewContent}
      </div>
      ${!this.insightPageEditing ? `<div id="relatedPapers" class="related-papers"></div>` : ''}
      <div class="insight-comments" id="insightCommentsPanel">
        <div class="comments-panel-header"><span>批注</span><span style="display:flex;align-items:center;gap:6px;"><span class="count" id="commentsCount">0</span><button class="comments-panel-close" onclick="app.hideCommentsPanel()" title="隐藏">×</button></span></div>
        <div class="comments-list" id="commentsList"></div>
        <div id="commentFormArea"></div>
      </div>`;
    const tInner0 = performance.now();
    document.getElementById('insightPage').innerHTML = html;
    const newBody = document.getElementById('insightPageBody');
    console.log('[perf]   innerHTML set:', (performance.now() - tInner0).toFixed(0), 'ms');

    // Only load full-res images when not busy (avoid flickering during generation)
    if (!isBusy) this.loadProgressiveImages();

    // Fetch insight HTML from backend (hybrid rendering)
    const needsBodyFetch = !isBusy && !this.insightPageEditing && p.insight && p.insight !== '__ANALYZING__';

    // Restore old insight body dimmed for smooth transition (loading spinner deferred by 3s)
    if (_oldBodyHtml && needsBodyFetch) {
      const newBody = document.getElementById('insightPageBody');
      if (newBody) {
        newBody.innerHTML = _oldBodyHtml;
        newBody.classList.add('insight-dimmed');
        this._updateThemeColor(document.documentElement.getAttribute('data-theme') === 'dark', true);
      }
    }

    this.loadRelatedPapers(p.id);
    this.loadComments(p.id, true);
    this.loadPaperTokens(p.id);
    if (needsBodyFetch) {
      this._fetchInsightHtml(p);
    }

    // Sync meta-info toggle arrow with saved preference
    const metaToggle = document.querySelector('.insight-meta-toggle');
    const metaInfo = document.getElementById('insightMetaInfo');
    if (metaToggle && metaInfo && metaInfo.style.display !== 'none') {
      metaToggle.style.transform = 'rotate(180deg)';
    }
    if (typeof requestIdleCallback !== 'undefined') {
      requestIdleCallback(() => this.renderDeferredMetaInfo(), { timeout: 3000 });
    } else {
      setTimeout(() => this.renderDeferredMetaInfo(), 0);
    }
    if (isBusy) {
      this.startInsightStream(p.id);
    } else {
      this.stopInsightStream();
    }
    const endBody = document.getElementById('insightPageBody');
  },

  _renderInsightMetaInfo(p, scoreClass) {
    const tagsHtml = p.tags && p.tags.length > 0
      ? p.tags.slice(0, 5).map(t => `<span class="insight-tag-chip">${this.escape(t.tag)}</span>`).join('')
      : '';
    const isOpen = localStorage.getItem('insightMetaInfoOpen') === '1';
    return `
      <div class="insight-meta-info" id="insightMetaInfo" style="display:${isOpen ? '' : 'none'};">
        <div class="insight-meta-info-inner">
          ${p.abstract ? `
          <div class="info-section">
            <div class="info-label">摘要</div>
            <div class="info-content dark" data-defer-markdown="1"></div>
          </div>` : ''}
          ${p.summary ? `
          <div class="info-section">
            <div class="info-label">总结</div>
            <div class="info-content dark" data-defer-markdown="1"></div>
          </div>` : ''}
          <div class="insight-meta-info-footer">
            <div class="meta-footer-left">${tagsHtml}</div>
            <div class="meta-footer-right"><span id="insightTokenCount" class="token-count"></span></div>
          </div>
        </div>
      </div>
    `;
  },

  toggleInsightMetaInfo() {
    const body = document.getElementById('insightMetaInfo');
    const icon = document.querySelector('.insight-meta-toggle');
    if (!body) return;
    const isOpen = body.style.display !== 'none';
    const willOpen = !isOpen;
    body.style.display = willOpen ? '' : 'none';
    localStorage.setItem('insightMetaInfoOpen', willOpen ? '1' : '0');
    if (icon) icon.style.transform = isOpen ? '' : 'rotate(180deg)';
    if (willOpen) this.renderDeferredMetaInfo();
  },

  toggleInsight(id, event) {
    if (this.pollingIds.has(id)) {
      this.showToast('该论文正在处理中，请稍候', 'info');
      return;
    }
    if (this.insightEditingIds.has(id)) {
      this.closeInsightDialog();
      return;
    }
    let paper = this.data && this.data.papers.find(p => p.id === id);
    if (!paper && this.insightPageData && this.insightPageData.id === id) {
      paper = this.insightPageData;
    }
    if (!paper) return;
    this.insightEditingIds.clear();
    this.insightEditingIds.add(id);
    document.getElementById('insightDialogTitle').textContent = '深度分析 · ' + paper.title;
    const ta = document.getElementById('insightDialogPrompt');
    ta.value = '';
    ta.readOnly = true;
    ta.placeholder = '加载中...';
    document.getElementById('insightDialogSubmit').disabled = false;
    document.getElementById('insightDialogSubmit').textContent = '生成';
    const meta = document.getElementById('insightDialogMeta');
    const loading = meta && meta.querySelector('.insight-loading-inline');
    if (loading) loading.remove();
    const providerSelect = document.getElementById('insightProviderSelect');
    if (providerSelect) {
      providerSelect.style.display = this.insightProviders.length > 1 ? '' : 'none';
    }
    this.openDialogWithOrigin('insightOverlay', event ? event.clientX : undefined, event ? event.clientY : undefined);
    this.trapFocus(document.getElementById('insightDialog'), null, true);
    const hint = document.getElementById('insightPromptHint');
    this.hideInsightDialogError();
    this.loadPrompts(this.selectedInsightProvider).then(prompts => {
      console.log('[toggleInsight] loadPrompts returned:', prompts);
      ta.readOnly = false;
      ta.placeholder = '';
      if (prompts) {
        ta.value = prompts.insight || '';
        if (hint) {
          console.log('[toggleInsight] calling updatePromptHint with hint=', hint, 'prompts=', prompts);
          this.updatePromptHint(hint, prompts);
        } else {
          console.log('[toggleInsight] hint element is null');
        }
      } else {
        console.log('[toggleInsight] prompts is null/falsy');
        this.showInsightDialogError('加载 prompt 失败，请重试');
      }
    }).catch(e => {
      ta.readOnly = false;
      ta.placeholder = '';
      this.showInsightDialogError('加载失败：' + (e.message || '网络错误'));
    });
  },

  showInsightDialogError(msg) {
    const el = document.getElementById('insightDialogError');
    if (!el) return;
    el.textContent = msg;
    el.classList.add('show');
  },

  hideInsightDialogError() {
    const el = document.getElementById('insightDialogError');
    if (!el) return;
    el.textContent = '';
    el.classList.remove('show');
  },

  closeInsightDialog() {
    this.untrapFocus(document.getElementById('insightDialog'));
    this.insightEditingIds.clear();
    this.closeDialogWithOrigin('insightOverlay', () => {
      const splitInput = document.getElementById('insightDialogSplitFigures');
      if (splitInput) splitInput.value = '';
      const autoReviewEl = document.getElementById('insightDialogAutoReview');
      if (autoReviewEl) autoReviewEl.checked = false;
      const keepCaptionEl = document.getElementById('insightDialogKeepCaption');
      if (keepCaptionEl) keepCaptionEl.checked = false;
      const skipValidationEl = document.getElementById('insightDialogSkipValidation');
      if (skipValidationEl) skipValidationEl.checked = false;
    });
  },

  cancelInsightDialog() {
    this.closeInsightDialog();
  },

  async generateInsight() {
    const id = [...this.insightEditingIds][0];
    if (!id) return;
    const prompt = document.getElementById('insightDialogPrompt').value.trim();
    const splitFigures = document.getElementById('insightDialogSplitFigures').value.trim();
    const autoReviewEl = document.getElementById('insightDialogAutoReview');
    const autoReview = autoReviewEl ? autoReviewEl.checked : true;
    const skipValidationEl = document.getElementById('insightDialogSkipValidation');
    const skipValidation = skipValidationEl ? skipValidationEl.checked : false;
    const keepCaptionEl = document.getElementById('insightDialogKeepCaption');
    const keepCaption = keepCaptionEl ? keepCaptionEl.checked : false;
    this.closeInsightDialog();
    let paper = this.data && this.data.papers.find(p => p.id === id);
    if (!paper && this.insightPageData && this.insightPageData.id === id) {
      paper = this.insightPageData;
    }
    const title = paper ? paper.title : '论文';
    this.showToast('', 'info', { html: this.escape(title) + '<br><span style="opacity:0.7;font-size:12px;">已放入队列，后台分析中...</span>' });
    // Mark as analyzing immediately for better UX
    if (paper) {
      paper.insight = '__ANALYZING__';
    }
    if (this.insightPageData && this.insightPageData.id === id) {
      this.insightPageData.insight = '__ANALYZING__';
      this.renderInsightPage();
    }
    if (this.data && this.data.papers) this._updateCardInsightUI(id, '__ANALYZING__', null);
    this.addPollingId(id);
    const headers = { 'Content-Type': 'application/json' };
    if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
    const body = { prompt: prompt || undefined };
    if (this.selectedInsightProvider) body.provider = this.selectedInsightProvider;
    if (splitFigures) body.split_figures = splitFigures;
    body.auto_review = autoReview;
    body.skip_validation = skipValidation;
    body.keep_caption = keepCaption;
    fetch('/api/papers/' + encodeURIComponent(this.paperSource(id)) + '/' + encodeURIComponent(id) + '/insight', {
      method: 'POST',
      headers,
      body: JSON.stringify(body),
    })
      .then(res => {
        if (!res.ok) throw new Error('insight failed');
        return res.json();
      })
      .then(updated => {
        if (this.data && this.data.papers) {
          const idx = this.data.papers.findIndex(p => p.id === id);
          if (idx >= 0) {
            this.data.papers[idx] = this.normalizePaper(updated);
          }
        }
        if (this.insightPageData && this.insightPageData.id === id) {
          this.insightPageData = this.normalizePaper(updated);
          this.renderInsightPage();
        }
        if (updated.insight === '__ANALYZING__') {
          // Background task started, keep polling
          if (this.data && this.data.papers) this._updateCardInsightUI(id, updated.insight, updated.insight_review);
          return;
        }
        // Completed immediately (unlikely but possible)
        this.removePollingId(id);
        if (this.data && this.data.papers) this._updateCardInsightUI(id, updated.insight, updated.insight_review);
      })
      .catch(() => {
        this.removePollingId(id);
        if (paper) paper.insight = '';
        if (this.insightPageData && this.insightPageData.id === id) {
          this.insightPageData.insight = '';
          this.renderInsightPage();
        }
        if (this.data && this.data.papers) this._updateCardInsightUI(id, '', null);
        this.showToast('深度分析失败，请重试', 'error');
      });
  },

  async loadPrompts(provider) {
    try {
      const url = provider ? `/api/prompts?provider=${encodeURIComponent(provider)}` : '/api/prompts';
      const res = await fetch(url);
      if (!res.ok) return null;
      return await res.json();
    } catch (e) {
      console.error('[loadPrompts]', e);
      return null;
    }
  },

  updatePromptHint(hintEl, prompts) {
    console.log('[updatePromptHint] called with hintEl=', hintEl, 'prompts=', prompts);
    if (!hintEl || !prompts) {
      console.log('[updatePromptHint] early return: hintEl=', hintEl, 'prompts=', prompts);
      return;
    }
    const variant = prompts.applied_variant;
    const mtime = prompts.applied_variant_mtime;
    console.log('[updatePromptHint] variant=', variant, 'mtime=', mtime);
    const icon = '<svg class="prompt-hint-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>';
    if (variant) {
      hintEl.innerHTML = `${icon}<span class="prompt-hint-name">已应用 ${this.escape(variant)} 模板</span>${mtime ? `<span class="prompt-hint-mtime">· 修改日期 ${this.escape(mtime)}</span>` : ''}`;
    } else {
      hintEl.innerHTML = `${icon}<span class="prompt-hint-name">已应用默认模板</span>${mtime ? `<span class="prompt-hint-mtime">· 修改日期 ${this.escape(mtime)}</span>` : ''}`;
    }
  },

  renderDeferredMetaInfo() {
    const p = this.insightPageData;
    if (!p) return;
    const metaInfo = document.getElementById('insightMetaInfo');
    if (!metaInfo) return;
    const sections = metaInfo.querySelectorAll('.info-section');
    sections.forEach(sec => {
      const label = sec.querySelector('.info-label');
      const content = sec.querySelector('.info-content.dark');
      if (!content || !content.dataset.deferMarkdown) return;
      const text = label ? label.textContent.trim() : '';
      if (text === '摘要' && p.abstract) {
        content.innerHTML = this.renderMarkdown(this.escapeHtml(p.abstract), false, { deferKatex: true, deferPrettier: true });
        content.removeAttribute('data-defer-markdown');
      } else if (text === '总结' && p.summary) {
        content.innerHTML = this.renderMarkdown(this.escapeHtml(p.summary), false, { deferKatex: true, deferPrettier: true });
        content.removeAttribute('data-defer-markdown');
      }
    });
    this.renderDeferredMath(metaInfo);
    this.renderDeferredPrettier(metaInfo);
  },

  loadProgressiveImages(container) {
    if (!container) container = document.getElementById('insightPage');
    if (!container) return;
    const imgs = container.querySelectorAll('img.progressive-img');
    imgs.forEach(img => {
      const fullSrc = img.dataset.fullSrc;
      if (!fullSrc || img.dataset.progressiveLoaded) return;
      img.dataset.progressiveLoaded = '1';
      // Load full image directly on the element so progressive JPEGs
      // render incrementally (top-to-bottom clarity) in place.
      img.addEventListener('load', () => img.classList.add('progressive-loaded'), { once: true });
      img.addEventListener('error', () => { /* keep thumb on error */ }, { once: true });
      img.src = fullSrc;
    });
  },
};
