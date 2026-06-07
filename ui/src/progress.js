// Auto-extracted from the original monolithic app.js.
// Each module exports a slice of the global `app` object; main.js merges them.
export const progress = {
  addPollingId(id) {
    this.pollingIds.add(id);
    if (!this.pollingTimer) {
      this.pollingTimer = setInterval(() => this.pollInsightStatus(), 5000);
    }
  },

  removePollingId(id) {
    this.pollingIds.delete(id);
    if (this.pollingIds.size === 0 && this.pollingTimer) {
      clearInterval(this.pollingTimer);
      this.pollingTimer = null;
    }
  },

  async pollInsightStatus() {
    if (this.pollingIds.size === 0) return;
    for (const id of [...this.pollingIds]) {
      try {
        const res = await fetch('/api/papers/' + encodeURIComponent(this.paperSource(id)) + '/' + encodeURIComponent(id));
        if (!res.ok) {
          if (res.status === 404) this.removePollingId(id);
          continue;
        }
        const paper = await res.json();
        if (this.data && this.data.papers) {
          const idx = this.data.papers.findIndex(p => p.id === id);
          if (idx >= 0) {
            this.data.papers[idx] = paper;
          }
        }
        const isAnalyzing = paper.insight === '__ANALYZING__';
        const isReviewing = paper.insight_review === '__REVIEWING__';
        if (!isAnalyzing && !isReviewing) {
          this.removePollingId(id);
          if (this.data && this.data.papers) this._updateCardInsightUI(id, paper.insight, paper.insight_review);
          // If currently viewing this paper's insight page and it was in a pending state, refresh it
          if (this.insightPageId === id && this.insightPageData) {
            const wasPending = this.insightPageData.insight === '__ANALYZING__' || this.insightPageData.insight_review === '__REVIEWING__';
            if (wasPending) {
              this.loadInsightPage(id);
            }
          }
        }
      } catch (e) { console.error('[pollInsightStatus]', e); }
    }
  },

  startProgressPolling() {
    if (this.progressTimer) return;
    this.fetchProgress();
    this.progressTimer = setInterval(() => this.fetchProgress(), 5000);
  },

  stopProgressPolling() {
    if (this.progressTimer) {
      clearInterval(this.progressTimer);
      this.progressTimer = null;
    }
    const panel = document.getElementById('insightProgressPanel');
    if (panel) panel.classList.remove('show');
  },

  _updateProgressHighlight() {
    const titles = document.getElementById('progressTitles');
    const completedTitles = document.getElementById('progressCompletedTitles');
    if (titles) {
      titles.querySelectorAll('.progress-title-item').forEach(el => {
        el.classList.toggle('active', el.getAttribute('onclick')?.includes("'" + this.insightPageId + "'") ?? false);
      });
    }
    if (completedTitles) {
      completedTitles.querySelectorAll('.progress-title-item').forEach(el => {
        el.classList.toggle('active', el.getAttribute('onclick')?.includes("'" + this.insightPageId + "'") ?? false);
      });
    }
  },

  async fetchProgress() {
    try {
      const res = await fetch('/api/insight-progress');
      if (!res.ok) return;
      const data = await res.json();
      this.updateProgressPanel(data);
    } catch (e) { /* silently ignore */ }
  },

  updateProgressPanel(data) {
    const panel = document.getElementById('insightProgressPanel');
    const analyzingSection = document.getElementById('progressAnalyzingSection');
    const badge = document.getElementById('progressBadge');
    const titles = document.getElementById('progressTitles');
    const completedHeader = document.getElementById('progressCompletedHeader');
    const completedTitles = document.getElementById('progressCompletedTitles');
    if (!panel || !analyzingSection || !badge || !titles || !completedHeader || !completedTitles) return;

    const currentItems = new Map();
    for (const t of (data.titles || [])) {
      currentItems.set(t.id, t.title);
    }

    // Detect newly completed papers (were analyzing before, now gone)
    for (const [id, title] of this.previousAnalyzingItems) {
      if (!currentItems.has(id)) {
        this.completedItems.push({ id: id, title: title, ts: Date.now() });
        // If currently viewing this paper's insight page, refresh it
        if (this.insightPageId === id && this.insightPageData && this.insightPageData.insight === '__ANALYZING__') {
          this.loadInsightPage(id);
        }
      }
    }
    this.previousAnalyzingItems = currentItems;

    const hasAnalyzing = currentItems.size > 0;
    const completedList = data.completed || [];

    // Render analyzing section
    if (hasAnalyzing) {
      analyzingSection.style.display = '';
      badge.textContent = currentItems.size;
      badge.style.display = '';
      titles.innerHTML = Array.from(currentItems).map(([id, title]) => {
        const activeClass = id === this.insightPageId ? ' active' : '';
        return '<div class="progress-title-item' + activeClass + '" onclick="app.showInsightPage(\'' + this.escape(id) + '\')" title="' + this.escape(title) + '"><div class="dot"></div><span class="progress-title-text">' + this.escape(title) + '</span>' + (this.authenticated ? '<span class="progress-cancel-btn" onclick="event.stopPropagation(); app.cancelInsight(\'' + this.escape(id) + '\')" title="取消分析">&times;</span>' : '') + '</div>';
      }).join('');
    } else {
      analyzingSection.style.display = 'none';
    }

    // Render completed section (always show recent 10 from DB)
    if (completedList.length > 0) {
      completedHeader.style.display = 'flex';
      completedHeader.style.borderTop = hasAnalyzing ? '' : 'none';
      completedHeader.style.marginTop = hasAnalyzing ? '' : '0';
      completedHeader.style.paddingTop = hasAnalyzing ? '' : '0';
      completedTitles.innerHTML = completedList.map(item => {
        const activeClass = item.id === this.insightPageId ? ' active' : '';
        return '<div class="progress-title-item completed' + activeClass + '" onclick="app.showInsightPage(\'' + this.escape(item.id) + '\')" title="' + this.escape(item.title) + '"><div class="dot"></div><span class="progress-title-text">' + this.escape(item.title) + '</span></div>';
      }).join('');
    } else {
      completedHeader.style.display = 'none';
      completedTitles.innerHTML = '';
    }

    // Handle collapsed state (vertical bar on left side)
    const bar = document.getElementById('progressBar');
    const lastCompletedAt = completedList.length > 0 ? completedList[0].processed_at : null;
    const isIdle = !hasAnalyzing && (!lastCompletedAt || Date.now() - lastCompletedAt > 60000);
    if (this.progressCollapsed) {
      panel.classList.remove('show');
      if (bar) {
        bar.classList.add('show');
        if (hasAnalyzing) {
          bar.classList.remove('done', 'idle');
          bar.classList.add('pulsing');
        } else if (isIdle) {
          bar.classList.remove('done', 'pulsing');
          bar.classList.add('idle');
        } else {
          bar.classList.remove('pulsing', 'idle');
          bar.classList.add('done');
        }
      }
    } else {
      panel.classList.add('show');
      if (bar) {
        bar.classList.remove('show');
        bar.classList.remove('pulsing');
      }
    }
  },

  toggleProgressCollapse() {
    this.haptic('light');
    this.progressCollapsed = !this.progressCollapsed;
    const panel = document.getElementById('insightProgressPanel');
    const bar = document.getElementById('progressBar');
    if (this.progressCollapsed) {
      if (panel) panel.classList.remove('show');
      if (bar) bar.classList.add('show');
    } else {
      if (panel) panel.classList.add('show');
      if (bar) bar.classList.remove('show');
      // Close TOC panel when opening progress panel
      const tocPanel = document.getElementById('insightTOC');
      const tocBar = document.getElementById('insightTocBar');
      if (tocPanel && tocPanel.classList.contains('show')) {
        tocPanel.classList.remove('show');
        if (tocBar) tocBar.classList.add('show');
        localStorage.setItem('insightTOCOpen', '0');
      }
    }
    // Refresh content in background
    this.fetchProgress();
  },
};
