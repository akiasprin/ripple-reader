// Auto-extracted from the original monolithic app.js.
// Each module exports a slice of the global `app` object; main.js merges them.
export const tags = {
  async loadTags() {
    try {
      // 1. Load dashboard (tags + counts) in one request
      const dashRes = await fetch('/api/tags/dashboard');
      const dash = await dashRes.json();
      this.tagsData = dash.tags || [];
      this.disinterestTags = dash.disinterest_tags || [];
      this.interestTags = dash.interest_tags || [];
      this.selectedDisinterestTags = new Set((this.disinterestTags || []).map(t => t.tag));
      // Render immediately with counts only
      this.renderTagsPage(dash.orphan_count || 0, dash.uninteresting_count || 0);

      // 2. Load full orphan/uninteresting lists in background (for drawer content)
      const [orphanRes, uninterestedRes] = await Promise.all([
        fetch('/api/papers/orphan-tags'),
        fetch('/api/papers/uninteresting'),
      ]);
      this.orphanData = await orphanRes.json();
      this.uninterestingData = await uninterestedRes.json();
    } catch (e) {
      document.getElementById('tagsPage').innerHTML = '<div class="empty">加载标签失败，请刷新重试</div>';
    }
  },

  renderTagsPage(orphanCount, uninterestingCount) {
    const el = document.getElementById('tagsPage');
    const rawTags = this.tagsData || [];
    const tags = rawTags.filter(t => t.count > 10);
    const orphanBtn = orphanCount !== undefined ? `(${orphanCount})` : '';
    const uninterestedBtn = uninterestingCount !== undefined ? `(${uninterestingCount})` : '';
    const grid = tags.map(t => `
      <div class="tag-card">
        <div class="tag-card-left">
          <a href="/?tag=${encodeURIComponent(t.tag)}" class="tag-card-name" onclick="event.preventDefault();app.filterByTag('${this.escape(t.tag)}');app.hideTagsPage();">${this.escape(t.tag)}</a>
          <div class="tag-card-count">${t.count} 篇论文</div>
        </div>
        <div class="tag-card-actions">
          ${this.authenticated ? `<button class="btn-danger" onclick="event.stopPropagation();app.deleteTagPapers('${this.escape(t.tag)}')">删除全部</button>` : ''}
        </div>
      </div>
    `).join('');
    el.innerHTML = `
      <div class="tags-layout">
        <div class="tags-main">
          <div class="tags-header">
            <h2>主题标签</h2>
            <div style="display:flex;gap:12px;align-items:center;">
              <span style="font-size:13px;color:var(--text3);">共 ${tags.length} 个标签（>10篇）</span>
              ${this.authenticated ? `
                <button class="btn-secondary" style="font-size:12px;padding:5px 12px;" onclick="app.showOrphanPanel()">孤立论文 ${orphanBtn}</button>
                <button class="btn-secondary" style="font-size:12px;padding:5px 12px;" onclick="app.showUninterestingPanel()">不感兴趣 ${uninterestedBtn}</button>
              ` : ''}
            </div>
          </div>
          ${tags.length === 0 ? '<div class="empty">暂无符合条件的标签</div>' : `<div class="tags-grid">${grid}</div>`}
        </div>
      </div>
    `;
  },

  showTagsPage() {
    this.tagsPageOpen = true;
    document.getElementById('filterBar').style.display = 'none';
    document.getElementById('stats').style.display = 'none';
    document.getElementById('list').style.display = 'none';
    document.getElementById('pagination').style.display = 'none';
    document.getElementById('insightPage').classList.remove('open');
    document.getElementById('tagsPage').classList.add('open');
    document.getElementById('navHome').classList.remove('active');
    document.getElementById('navTags').classList.add('active');
    window.scrollTo({ top: 0, behavior: 'instant' });
    this.hideOrphanPanel();
    this.loadTags();
  },

  hideTagsPage() {
    this.tagsPageOpen = false;
    document.getElementById('tagsPage').classList.remove('open');
    document.getElementById('tagsPage').innerHTML = '';
    document.getElementById('filterBar').style.display = '';
    document.getElementById('stats').style.display = '';
    document.getElementById('list').style.display = '';
    document.getElementById('pagination').style.display = '';
    document.getElementById('navHome').classList.add('active');
    document.getElementById('navTags').classList.remove('active');
    this.hideOrphanPanel();
  },

  async deleteTagPapers(tag) {
    if (!this.authenticated) return;
    if (!confirm(`确定要删除标签「${tag}」下的所有论文吗？`)) return;
    try {
      const headers = {};
      if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
      const res = await fetch('/api/tags/' + encodeURIComponent(tag) + '/papers', {
        method: 'DELETE',
        headers,
      });
      if (!res.ok) {
        this.showToast('删除失败', 'error');
        return;
      }
      const data = await res.json();
      this.showToast(`已删除 ${data.deleted} 篇论文`, 'success');
      this.data = null; // Force reload on next list view
      this.loadTags();
    } catch (e) {
      this.showToast('删除失败', 'error');
    }
  },

  renderTags(p) {
    if (!p.tags || p.tags.length === 0) return '';
    const visible = p.tags.slice(0, 3);
    return '（' + visible.map(t => `<span class="tag-link">${this.escape(t.tag)}</span>`).join('、') + '）';
  },

  async filterByTag(tag) {
    this.page = 1;
    this.tagFilter = tag;
    this.pushURL();
    await this.load();
    window.scrollTo(0, 0);
  },

  async clearTagFilter() {
    this.page = 1;
    this.tagFilter = '';
    this.pushURL();
    await this.load();
    window.scrollTo(0, 0);
  },
};
