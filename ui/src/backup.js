// Auto-extracted from the original monolithic app.js.
// Each module exports a slice of the global `app` object; main.js merges them.
export const backup = {
  async showInsightBackups(id, event) {
    const headers = {};
    if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
    try {
      const res = await fetch('/api/papers/' + encodeURIComponent(this.paperSource(id)) + '/' + encodeURIComponent(id) + '/insight-backups', { headers });
      if (!res.ok) { this.showToast('加载备份失败', 'error'); return; }
      const data = await res.json();
      const list = data.backups || [];
      if (list.length === 0) { this.showToast('暂无备份', 'info'); return; }
      this._currentBackups = list;
      let html = '<div class="backup-list">';
      for (let idx = 0; idx < list.length; idx++) {
        const b = list[idx];
        const num = list.length - idx;
        const date = this.formatDate(b.created_at);
        const hasReview = b.insight_review ? '<span class="backup-review-tag">Review</span>' : '';
        const modelMatch = (b.insight || '').match(/本文使用\s*(.+?)\s*模型生成/);
        const modelTag = modelMatch ? `<span class="backup-model-tag">${this.escape(modelMatch[1])}</span>` : '';
        const preview = b.insight.substring(0, 90).replace(/\n/g, ' ') + (b.insight.length > 90 ? '...' : '');
        html += `
          <div class="backup-item" data-backup-id="${b.id}">
            <div class="backup-item-body">
              <div class="backup-item-preview">${this.escape(preview)}</div>
              <div class="backup-item-meta"><span class="backup-item-num">#${num}</span><span class="backup-item-date">${date}</span>${modelTag}${hasReview}</div>
            </div>
            <div class="backup-item-actions">
              <button class="backup-restore-btn" onclick="app.confirmRestoreBackup('${this.escape(id)}', ${b.id}, event)">还原</button>
              <button class="backup-delete-btn" onclick="app.deleteInsightBackup('${this.escape(id)}', ${b.id})">删除</button>
            </div>
          </div>`;
      }
      html += '</div>';
      this.restoreDialog({ title: 'Insight 备份', body: html }, event ? event.clientX : undefined, event ? event.clientY : undefined);
    } catch (e) {
      console.error('[showInsightBackups]', e);
      this.showToast('加载备份失败', 'error');
    }
  },

  async restoreInsightBackup(id, backupId) {
    const headers = { 'Content-Type': 'application/json' };
    if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
    try {
      const res = await fetch('/api/papers/' + encodeURIComponent(this.paperSource(id)) + '/' + encodeURIComponent(id) + '/insight-backups', {
        method: 'POST',
        headers,
        body: JSON.stringify({ backup_id: backupId }),
      });
      if (!res.ok) { this.showToast('还原失败', 'error'); return; }
      const updated = await res.json();
      this.insightPageData = updated;
      this.insightPageEditing = false;
      this.renderInsightPage();
      this.hideModal();
      this.showToast('已还原备份', 'success');
    } catch (e) {
      console.error('[restoreInsightBackup]', e);
      this.showToast('还原失败', 'error');
    }
  },

  async deleteInsightBackup(id, backupId) {
    if (!confirm('确定删除该备份？')) return;
    const headers = {};
    if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
    try {
      const res = await fetch('/api/papers/' + encodeURIComponent(this.paperSource(id)) + '/' + encodeURIComponent(id) + '/insight-backups/' + backupId, {
        method: 'DELETE',
        headers,
      });
      if (!res.ok) { this.showToast('删除备份失败', 'error'); return; }
      this.showToast('已删除备份', 'success');
      this._currentBackups = (this._currentBackups || []).filter(b => b.id !== backupId);
      const listEl = document.querySelector('.backup-list');
      if (listEl) {
        const item = listEl.querySelector(`[data-backup-id="${backupId}"]`);
        if (item) {
          item.style.opacity = '0';
          item.style.transform = 'scale(0.96)';
          setTimeout(() => item.remove(), 200);
        }
      }
      if ((this._currentBackups || []).length === 0) this.hideModal();
    } catch (e) {
      console.error('[deleteInsightBackup]', e);
      this.showToast('删除备份失败', 'error');
    }
  },

  closeBackupDiff() {
    const overlay = document.getElementById('backupDiffOverlay');
    if (!overlay) return;
    this.untrapFocus(overlay);
    if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
      if (overlay.parentNode) overlay.remove();
      if (document.fullscreenElement) document.exitFullscreen();
      return;
    }
    const originX = overlay._originX;
    const originY = overlay._originY;
    overlay.style.transformOrigin = `${originX !== undefined ? originX : overlay.offsetWidth / 2}px ${originY !== undefined ? originY : overlay.offsetHeight / 2}px`;
    overlay.style.willChange = 'transform, opacity';
    const anim = overlay.animate([
      { transform: 'scale(1)', opacity: 1 },
      { transform: 'scale(0)', opacity: 0 }
    ], {
      duration: 200,
      easing: 'ease-in',
      fill: 'forwards'
    });
    anim.onfinish = () => {
      if (overlay.parentNode) overlay.remove();
      if (document.fullscreenElement) document.exitFullscreen();
    };
  },

  toggleBackupDiffFullscreen() {
    if (document.fullscreenElement) {
      document.exitFullscreen();
    } else {
      const el = document.getElementById('backupDiffOverlay');
      if (el) el.requestFullscreen();
    }
  },

  async confirmRestoreBackup(paperId, backupId, event) {
    const backup = (this._currentBackups || []).find(b => b.id === backupId);
    if (!backup) { this.showToast('备份数据不存在', 'error'); return; }
    const headers = {};
    if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
    let paper;
    if (this.insightPageId === paperId && this.insightPageData) {
      paper = this.insightPageData;
    } else {
      try {
        const res = await fetch('/api/papers/' + encodeURIComponent(this.paperSource(paperId)) + '/' + encodeURIComponent(paperId), { headers });
        if (!res.ok) { this.showToast('加载当前版本失败', 'error'); return; }
        paper = await res.json();
      } catch (e) {
        console.error('[confirmRestoreBackup]', e);
        this.showToast('加载当前版本失败', 'error'); return;
      }
    }
    this._renderRestoreConfirm(paper, backup, event ? event.clientX : undefined, event ? event.clientY : undefined);
  },

  _renderRestoreConfirm(paper, backup, clickX, clickY) {
    const currentHtml = this.renderMarkdown(this.escapeHtml(paper.insight || ''), true);
    const backupHtml = this.renderMarkdown(this.escapeHtml(backup.insight || ''), true);
    const currentWords = (paper.insight || '').length;
    const backupWords = (backup.insight || '').length;
    const leftTitle = '当前版本 · ' + currentWords + ' 字';
    const rightTitle = '备份版本 · ' + this.formatDate(backup.created_at) + ' · ' + backupWords + ' 字';
    const overlay = document.createElement('div');
    overlay.id = 'backupDiffOverlay';
    overlay.className = 'backup-diff-overlay';
    overlay.innerHTML = '<div class="backup-diff-header">' +
      '<div class="backup-diff-title">' +
        '<span>还原确认 · ' + this.escape(paper.title) + '</span>' +
      '</div>' +
      '<div class="backup-diff-actions">' +
        '<span class="backup-diff-hint">恢复前已自动备份当前版本</span>' +
        '<button class="btn-secondary" onclick="app.closeBackupDiff()">取消</button>' +
        '<button class="btn-primary" onclick="app.doRestoreBackup(\'' + this.escape(paper.id) + '\', ' + backup.id + ')">确认恢复</button>' +
      '</div>' +
    '</div>' +
    '<div class="backup-diff-body">' +
      '<div class="backup-diff-col">' +
        '<div class="backup-diff-col-header">' +
          '<span>' + leftTitle + '</span>' +
          '<span class="version-tag current">当前</span>' +
        '</div>' +
        '<div class="backup-diff-col-body insight-page-body">' + currentHtml + '</div>' +
      '</div>' +
      '<div class="backup-diff-col">' +
        '<div class="backup-diff-col-header">' +
          '<span>' + rightTitle + '</span>' +
          '<span class="version-tag backup">备份</span>' +
        '</div>' +
        '<div class="backup-diff-col-body insight-page-body">' + backupHtml + '</div>' +
      '</div>' +
    '</div>' +
    '<div class="backup-diff-footer">' +
      '<button class="btn-secondary" onclick="app.closeBackupDiff()">取消</button>' +
      '<button class="btn-primary" onclick="app.doRestoreBackup(\'' + this.escape(paper.id) + '\', ' + backup.id + ')">确认恢复</button>' +
    '</div>';
    document.body.appendChild(overlay);
    overlay._originX = clickX;
    overlay._originY = clickY;
    this._animateOverlayOpen(overlay, clickX, clickY);
    this.trapFocus(overlay);
  },

  async doRestoreBackup(id, backupId) {
    const headers = { 'Content-Type': 'application/json' };
    if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
    try {
      const res = await fetch('/api/papers/' + encodeURIComponent(this.paperSource(id)) + '/' + encodeURIComponent(id) + '/insight-backups', {
        method: 'POST',
        headers,
        body: JSON.stringify({ backup_id: backupId }),
      });
      if (!res.ok) { this.showToast('还原失败', 'error'); return; }
      const updated = await res.json();
      this.insightPageData = updated;
      this.insightPageEditing = false;
      this.renderInsightPage();
      this.hideModal();
      this.closeBackupDiff();
      this.showToast('已还原备份', 'success');
    } catch (e) {
      console.error('[doRestoreBackup]', e);
      this.showToast('还原失败', 'error');
    }
  },

  _computeLineDiff(oldText, newText) {
    const a = oldText.split('\n');
    const b = newText.split('\n');
    const result = [];
    let i = 0, j = 0;
    while (i < a.length || j < b.length) {
      if (i < a.length && j < b.length && a[i] === b[j]) {
        result.push({ type: 'same', left: a[i], right: b[j] });
        i++; j++;
      } else {
        let found = false;
        const MAX = 15;
        for (let k = 1; k <= MAX && !found; k++) {
          if (i + k < a.length && j < b.length && a[i + k] === b[j]) {
            for (let m = 0; m < k; m++) result.push({ type: 'del', left: a[i + m], right: null });
            i += k; found = true;
          } else if (j + k < b.length && i < a.length && a[i] === b[j + k]) {
            for (let m = 0; m < k; m++) result.push({ type: 'ins', left: null, right: b[j + m] });
            j += k; found = true;
          } else if (i + k < a.length && j + k < b.length && a[i + k] === b[j + k]) {
            for (let m = 0; m < k; m++) result.push({ type: 'del', left: a[i + m], right: null });
            for (let m = 0; m < k; m++) result.push({ type: 'ins', left: null, right: b[j + m] });
            i += k; j += k; found = true;
          }
        }
        if (!found) {
          if (i < a.length && j < b.length) {
            result.push({ type: 'del', left: a[i], right: null }); i++;
            result.push({ type: 'ins', left: null, right: b[j] }); j++;
          } else if (i < a.length) {
            result.push({ type: 'del', left: a[i], right: null }); i++;
          } else {
            result.push({ type: 'ins', left: null, right: b[j] }); j++;
          }
        }
      }
    }
    return result;
  },
};
