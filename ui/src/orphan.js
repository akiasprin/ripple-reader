// Auto-extracted from the original monolithic app.js.
// Each module exports a slice of the global `app` object; main.js merges them.
export const orphan = {
  showOrphanPanel() {
    this.renderOrphanPanel();
    const drawer = document.getElementById('orphanDrawer');
    drawer.classList.add('open');
    document.getElementById('orphanOverlay').classList.add('open');
    this.trapFocus(drawer);
  },

  hideOrphanPanel() {
    this.untrapFocus(document.getElementById('orphanDrawer'));
    document.getElementById('orphanDrawer').classList.remove('open');
    document.getElementById('orphanOverlay').classList.remove('open');
  },

  renderOrphanPanel() {
    document.getElementById('orphanDrawerTitle').textContent = '孤立论文';
    const body = document.getElementById('orphanDrawerBody');
    const footer = document.getElementById('orphanDrawerFooter');
    const orphans = this.orphanData || [];
    if (orphans.length === 0) {
      body.innerHTML = '<div class="empty">没有孤立论文</div>';
      footer.innerHTML = '';
      return;
    }
    const cards = orphans.map(o => {
      const tagsHtml = (o.tags || []).slice(0, 5).map(t => `<span class="tag-link">${this.escape(t.tag)}</span>`).join(' ');
      return `
        <div class="orphan-card">
          <div class="orphan-card-header">
            <input type="checkbox" class="orphan-check" value="${this.escape(o.id)}" id="orphan-${this.escape(o.id)}">
            <span class="orphan-card-title" onclick="app.showInsightPage('${this.escape(o.id)}')">${this.escape(o.title)}</span>
          </div>
          ${tagsHtml ? `<div class="orphan-card-tags">${tagsHtml}</div>` : ''}
          <div class="orphan-card-abstract">${this.escape(o.abstract_zh || '')}</div>
        </div>
      `;
    }).join('');
    body.innerHTML = cards;
    footer.innerHTML = `
      <button onclick="app.toggleSelectAllOrphans()">全选</button>
      <button class="btn-danger" onclick="app.batchDeleteOrphans()">批量删除</button>
    `;
  },

  showUninterestingPanel() {
    this.renderUninterestingPanel();
    const drawer = document.getElementById('orphanDrawer');
    drawer.classList.add('open');
    document.getElementById('orphanOverlay').classList.add('open');
    this.trapFocus(drawer);
  },

  renderUninterestingPanel() {
    document.getElementById('orphanDrawerTitle').textContent = '不感兴趣论文';
    const body = document.getElementById('orphanDrawerBody');
    const footer = document.getElementById('orphanDrawerFooter');
    const interestSet = new Set(this.interestTags || []);
    const disinterestMap = new Map((this.disinterestTags || []).map(t => [t.tag, t.count]));
    // All non-interest tags from tagsData
    const allTags = (this.tagsData || []).filter(t => !interestSet.has(t.tag));
    if (allTags.length === 0) {
      body.innerHTML = '<div class="empty">没有可用标签（全部被标记为感兴趣）</div>';
      footer.innerHTML = '';
      return;
    }
    const disinterestTags = allTags.filter(t => disinterestMap.has(t.tag));
    const neutralTags = allTags.filter(t => !disinterestMap.has(t.tag));
    const renderTag = (t, cls) => {
      const checked = this.selectedDisinterestTags.has(t.tag) ? 'checked' : '';
      const count = disinterestMap.has(t.tag) ? disinterestMap.get(t.tag) : t.count;
      return `
        <label class="${cls}">
          <input type="checkbox" value="${this.escape(t.tag)}" onchange="app.toggleDisinterestTag('${this.escape(t.tag)}')" ${checked}>
          <span>${this.escape(t.tag)}</span>
          <span class="disinterest-tag-count">(${count})</span>
        </label>
      `;
    };
    const disinterestHtml = disinterestTags.length > 0
      ? `<div style="font-size:12px;color:#dc2626;margin-bottom:6px;font-weight:600;">不感兴趣</div><div class="disinterest-tags-list">${disinterestTags.map(t => renderTag(t, 'disinterest-tag-check')).join('')}</div>`
      : '';
    const neutralHtml = neutralTags.length > 0
      ? `<div style="font-size:12px;color:var(--text3);margin:10px 0 6px;font-weight:600;">中性标签</div><div class="disinterest-tags-list">${neutralTags.map(t => renderTag(t, 'neutral-tag-check')).join('')}</div>`
      : '';
    const selectedTags = Array.from(this.selectedDisinterestTags);
    let papersHtml = '';
    if (selectedTags.length > 0) {
      const papers = (this.uninterestingData || []).filter(p => {
        const paperTags = (p.tags || []).map(t => t.tag);
        return selectedTags.some(st => paperTags.some(pt => pt.includes(st)));
      });
      if (papers.length > 0) {
        const paperCards = papers.map(p => {
          const tagsHtml = (p.tags || []).slice(0, 5).map(t => `<span class="tag-link">${this.escape(t.tag)}</span>`).join(' ');
          return `
            <div class="orphan-card">
              <div class="orphan-card-header">
                <span class="orphan-card-title" onclick="app.showInsightPage('${this.escape(p.id)}')">${this.escape(p.title)}</span>
              </div>
              ${tagsHtml ? `<div class="orphan-card-tags">${tagsHtml}</div>` : ''}
              <div class="orphan-card-abstract">${this.escape(p.abstract_zh || '')}</div>
            </div>
          `;
        }).join('');
        papersHtml = `
          <div style="margin-top:16px;border-top:1px solid var(--border);padding-top:12px;">
            <div style="font-size:13px;color:var(--text2);margin-bottom:8px;">匹配论文 (${papers.length} 篇)</div>
            ${paperCards}
          </div>
        `;
      } else {
        papersHtml = `<div style="margin-top:16px;border-top:1px solid var(--border);padding-top:12px;"><div class="empty">没有匹配论文</div></div>`;
      }
    }
    body.innerHTML = `
      <div style="font-size:13px;color:var(--text2);margin-bottom:8px;">选择标签（将删除匹配论文）：</div>
      ${disinterestHtml}
      ${neutralHtml}
      ${papersHtml}
    `;
    footer.innerHTML = `
      <button onclick="app.selectAllDisinterestTags(true)">全选</button>
      <button onclick="app.selectAllDisinterestTags(false)">全不选</button>
      <button class="btn-secondary" onclick="app.renderUninterestingPanel()">查看论文</button>
      <button class="btn-danger" onclick="app.batchDeleteUninteresting()">批量删除</button>
    `;
  },

  toggleDisinterestTag(tag) {
    if (this.selectedDisinterestTags.has(tag)) {
      this.selectedDisinterestTags.delete(tag);
    } else {
      this.selectedDisinterestTags.add(tag);
    }
    this.renderUninterestingPanel();
  },

  selectAllDisinterestTags(select) {
    if (select) {
      const interestSet = new Set(this.interestTags || []);
      const allNonInterest = (this.tagsData || []).filter(t => !interestSet.has(t.tag));
      this.selectedDisinterestTags = new Set(allNonInterest.map(t => t.tag));
    } else {
      this.selectedDisinterestTags.clear();
    }
    this.renderUninterestingPanel();
  },

  async batchDeleteUninteresting() {
    if (!this.authenticated) return;
    const selectedTags = Array.from(this.selectedDisinterestTags);
    if (selectedTags.length === 0) {
      this.showToast('请至少选择一个不感兴趣标签', 'error');
      return;
    }
    const allTagsMap = new Map((this.tagsData || []).map(t => [t.tag, t.count]));
    const totalCount = selectedTags.reduce((sum, tag) => sum + (allTagsMap.get(tag) || 0), 0);
    const confirmMsg = `确定要删除匹配以下 ${selectedTags.length} 个标签的论文吗？\n\n${selectedTags.join('、')}\n\n预计影响 ${totalCount} 篇论文（关键论文将自动跳过）`;
    if (!confirm(confirmMsg)) return;
    const headers = { 'Content-Type': 'application/json' };
    if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
    try {
      const res = await fetch('/api/papers/uninteresting', {
        method: 'DELETE',
        headers,
        body: JSON.stringify({ tags: selectedTags }),
      });
      if (!res.ok) throw new Error('删除失败');
      const result = await res.json();
      let msg = `已删除 ${result.deleted || 0} 篇论文`;
      if (result.skipped > 0) msg += `，跳过 ${result.skipped} 篇关键论文`;
      this.showToast(msg, 'success');
      this.data = null; // Force reload on next list view
      await this.loadTags();
      this.renderUninterestingPanel();
    } catch (e) {
      this.showToast('批量删除失败', 'error');
    }
  },

  toggleSelectAllOrphans() {
    const checks = document.querySelectorAll('.orphan-check');
    if (checks.length === 0) return;
    const allChecked = Array.from(checks).every(c => c.checked);
    checks.forEach(c => c.checked = !allChecked);
  },

  async batchDeleteOrphans() {
    if (!this.authenticated) return;
    const checks = document.querySelectorAll('.orphan-check:checked');
    const ids = Array.from(checks).map(c => c.value);
    if (ids.length === 0) {
      this.showToast('请先勾选要删除的论文', 'error');
      return;
    }
    const orphans = this.orphanData || [];
    const toDelete = [];
    let skipped = 0;
    for (const id of ids) {
      const p = orphans.find(o => o.id === id);
      if (p && p.mark === 'critical') {
        skipped++;
        continue;
      }
      toDelete.push(id);
    }
    if (toDelete.length === 0) {
      this.showToast(`所选 ${skipped} 篇均为关键论文，已跳过`, 'error');
      return;
    }
    let confirmMsg = `确定要删除选中的 ${toDelete.length} 篇孤立论文吗？`;
    if (skipped > 0) confirmMsg += `（${skipped} 篇关键论文将跳过）`;
    if (!confirm(confirmMsg)) return;
    const headers = {};
    if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
    let deleted = 0;
    for (const id of toDelete) {
      try {
        const orphan = orphans.find(o => o.id === id);
        const source = orphan && orphan.source_type ? orphan.source_type : 'arxiv';
        const res = await fetch('/api/papers/' + encodeURIComponent(source) + '/' + encodeURIComponent(id), {
          method: 'DELETE',
          headers,
        });
        if (res.ok) deleted++;
      } catch (e) {}
    }
    let msg = `已删除 ${deleted} 篇论文`;
    if (skipped > 0) msg += `，跳过 ${skipped} 篇关键论文`;
    this.showToast(msg, 'success');
    // Remove deleted papers from list data
    if (this.data && this.data.papers) {
      for (const id of toDelete) {
        const idx = this.data.papers.findIndex(p => p.id === id);
        if (idx >= 0) {
          this.data.papers.splice(idx, 1);
          this.data.total -= 1;
        }
      }
    }
    await this.loadTags();
    this.renderOrphanPanel();
  },
};
