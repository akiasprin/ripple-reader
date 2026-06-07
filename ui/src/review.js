// Auto-extracted from the original monolithic app.js.
// Each module exports a slice of the global `app` object; main.js merges them.
export const review = {
  reviewInsight(id, event) {
    let paper = this.data && this.data.papers.find(p => p.id === id);
    if (!paper && this.insightPageData && this.insightPageData.id === id) {
      paper = this.insightPageData;
    }
    if (!paper) return;
    this.openReviewDialog(id, paper.title, event);
  },

  openReviewDialog(id, title, event) {
    this.reviewEditingIds.clear();
    this.reviewEditingIds.add(id);
    document.getElementById('reviewDialogTitle').textContent = '审查 Insight · ' + title;
    const ta = document.getElementById('reviewDialogPrompt');
    ta.value = '';
    ta.readOnly = true;
    ta.placeholder = '加载中...';
    document.getElementById('reviewDialogSubmit').disabled = false;
    document.getElementById('reviewDialogSubmit').textContent = '审查';
    const providerSelect = document.getElementById('reviewProviderSelect');
    if (providerSelect) {
      providerSelect.style.display = this.insightProviders.length > 1 ? '' : 'none';
      const dropdown = document.getElementById('reviewProviderDropdown');
      if (dropdown) {
        dropdown.innerHTML = this.insightProviders.map(p =>
          `<option value="${this.escape(p)}" ${p === this.selectedInsightProvider ? 'selected' : ''}>${this.escape(p)}</option>`
        ).join('');
      }
    }
    this.openDialogWithOrigin('reviewOverlay', event ? event.clientX : undefined, event ? event.clientY : undefined);
    this.trapFocus(document.getElementById('reviewDialog'));
    const hint = document.getElementById('reviewPromptHint');
    this.loadPrompts(this.selectedInsightProvider).then(prompts => {
      ta.readOnly = false;
      ta.placeholder = '';
      if (prompts) {
        ta.value = prompts.review || '';
        if (hint) this.updatePromptHint(hint, prompts);
      }
      ta.focus();
    });
  },

  closeReviewDialog() {
    this.untrapFocus(document.getElementById('reviewDialog'));
    this.reviewEditingIds.clear();
    this.closeDialogWithOrigin('reviewOverlay', () => {
      const ta = document.getElementById('reviewDialogPrompt');
      if (ta) ta.value = '';
    });
  },

  cancelReview() {
    this.closeReviewDialog();
  },

  async cancelReviewTask(id) {
    const headers = {};
    if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
    try {
      const res = await fetch('/api/papers/' + encodeURIComponent(this.paperSource(id)) + '/' + encodeURIComponent(id) + '/review', {
        method: 'DELETE',
        headers,
      });
      if (!res.ok) {
        this.showToast('取消审核失败', 'error');
        return;
      }
      const updated = await res.json();
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
        this.renderInsightPage();
      }
      this.showToast('已取消审核', 'success');
    } catch (e) {
      console.error('[cancelReviewTask]', e);
      this.showToast('取消审核失败', 'error');
    }
  },

  async submitReview() {
    const id = [...this.reviewEditingIds][0];
    if (!id) return;
    const prompt = document.getElementById('reviewDialogPrompt').value.trim();
    this.closeReviewDialog();
    let paper = this.data && this.data.papers.find(p => p.id === id);
    if (!paper && this.insightPageData && this.insightPageData.id === id) {
      paper = this.insightPageData;
    }
    const title = paper ? paper.title : '论文';
    this.showToast('', 'info', { html: this.escape(title) + '<br><span style="opacity:0.7;font-size:12px;">审查已放入队列，后台运行中...</span>' });
    // Save pre-optimistic state for rollback on failure
    const prevReview = paper ? paper.insight_review : null;
    const prevReviewedAt = paper ? paper.insight_reviewed_at : null;
    const prevPageReview = this.insightPageData && this.insightPageData.id === id ? this.insightPageData.insight_review : null;
    const prevPageReviewedAt = this.insightPageData && this.insightPageData.id === id ? this.insightPageData.insight_reviewed_at : null;
    if (paper) {
      paper.insight_review = '__REVIEWING__';
      paper.insight_reviewed_at = null;
    }
    if (this.insightPageData && this.insightPageData.id === id) {
      this.insightPageData.insight_review = '__REVIEWING__';
      this.insightPageData.insight_reviewed_at = null;
      this.renderInsightPage();
    }
    if (this.data && this.data.papers) this.renderList();
    this.addPollingId(id);
    const rollback = () => {
      if (paper) {
        paper.insight_review = prevReview;
        paper.insight_reviewed_at = prevReviewedAt;
      }
      if (this.insightPageData && this.insightPageData.id === id) {
        this.insightPageData.insight_review = prevPageReview;
        this.insightPageData.insight_reviewed_at = prevPageReviewedAt;
        this.renderInsightPage();
      }
      if (this.data && this.data.papers) this.renderList();
    };
    const headers = { 'Content-Type': 'application/json' };
    if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
    const body = {};
    if (this.selectedInsightProvider) body.provider = this.selectedInsightProvider;
    if (prompt) body.prompt = prompt;
    try {
      const res = await fetch('/api/papers/' + encodeURIComponent(this.paperSource(id)) + '/' + encodeURIComponent(id) + '/review', {
        method: 'POST',
        headers,
        body: JSON.stringify(body),
      });
      if (!res.ok) {
        rollback();
        if (res.status === 400) {
          this.showToast('没有可审查的深度分析内容', 'error');
        } else if (res.status === 409) {
          this.showToast('该论文正在审查中', 'info');
        } else {
          this.showToast('审查启动失败', 'error');
        }
        this.removePollingId(id);
        return;
      }
      const updated = await res.json();
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
      if (this.data && this.data.papers) this.renderList();
    } catch (e) {
      rollback();
      console.error('[submitReview]', e);
      this.showToast('审查启动失败', 'error');
      this.removePollingId(id);
    }
  },
};
