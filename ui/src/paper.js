// Auto-extracted from the original monolithic app.js.
// Each module exports a slice of the global `app` object; main.js merges them.
export const paper = {
  startEdit(id) {
    this.editingIds.add(id);
    this.renderList();
    setTimeout(() => {
      const card = document.querySelector(`.paper-card[data-id="${CSS.escape(id)}"]`);
      if (card) card.scrollIntoView({ behavior: 'smooth', block: 'start' });
    }, 0);
  },

  cancelEdit(id) {
    this.editingIds.delete(id);
    this.renderList();
  },

  async save(id) {
    const updates = {
      title: document.getElementById('f-title-' + id).value,
      score: parseFloat(document.getElementById('f-score-' + id).value),
      paper_type: document.getElementById('f-type-' + id).value,
      abstract: document.getElementById('f-abstract-' + id).value,
      summary: document.getElementById('f-summary-' + id).value,
    };
    try {
      const headers = { 'Content-Type': 'application/json' };
      if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
      const res = await fetch('/api/papers/' + encodeURIComponent(this.paperSource(id)) + '/' + encodeURIComponent(id), {
        method: 'PUT',
        headers,
        body: JSON.stringify(updates),
      });
      if (!res.ok) throw new Error('save failed');
      const updated = await res.json();
      const idx = this.data.papers.findIndex(p => p.id === id);
      if (idx >= 0) this.data.papers[idx] = this.normalizePaper(updated);
      this.editingIds.delete(id);
      this.renderList();
    } catch (e) {
      console.error('[savePaper]', e);
      alert('保存失败');
    }
  },

  async deletePaper(id) {
    try {
      const headers = {};
      if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
      const idx = this.data.papers.findIndex(p => p.id === id);
      const deletedPaper = idx >= 0 ? this.data.papers[idx] : null;
      const res = await fetch('/api/papers/' + encodeURIComponent(this.paperSource(id)) + '/' + encodeURIComponent(id), {
        method: 'DELETE',
        headers,
      });
      if (!res.ok) throw new Error('delete failed');
      this.editingIds.delete(id);
      if (idx >= 0) {
        this.data.papers.splice(idx, 1);
        this.data.total -= 1;
      }
      this.renderList();
      // Show toast with undo action
      const paperTitle = deletedPaper ? this.escape(deletedPaper.title) : id;
      const toastHtml = '<div class="toast-title">已删除「' + paperTitle + '」</div>' +
        '<div class="toast-actions"><button data-action="undo">撤回删除</button></div>';
      this.showToast('', 'success', {
        html: toastHtml,
        duration: 8000,
        onAction: async () => {
          try {
            const restoreRes = await fetch('/api/papers/' + encodeURIComponent(this.paperSource(id)) + '/' + encodeURIComponent(id) + '/restore', {
              method: 'POST',
              headers,
            });
            if (!restoreRes.ok) throw new Error('restore failed');
            // Reload to get the paper back
            await this.load();
            this.showToast('已撤回删除', 'info');
          } catch (err) {
            console.error('[restorePaper]', err);
            this.showToast('撤回失败', 'error');
          }
        }
      });
    } catch (e) {
      console.error('[deletePaper]', e);
      alert('删除失败');
    }
  },

  async setMark(id, mark) {
    this.haptic('medium');
    try {
      const headers = { 'Content-Type': 'application/json' };
      if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
      const res = await fetch('/api/papers/' + encodeURIComponent(this.paperSource(id)) + '/' + encodeURIComponent(id) + '/mark', {
        method: 'PUT',
        headers,
        body: JSON.stringify({ mark }),
      });
      if (!res.ok) throw new Error('mark failed');
      const updated = await res.json();
      const idx = this.data.papers.findIndex(p => p.id === id);
      if (idx >= 0) {
        this.data.papers[idx] = this.normalizePaper(updated);
        this._updateCardMarkUI(id, mark);
      }
    } catch (e) {
      console.error('[toggleMark]', e);
      alert('标记失败');
    }
  },

  _updateCardMarkUI(id, mark) {
    const card = document.querySelector(`.paper-card[data-id="${CSS.escape(id)}"]`);
    if (!card) return;

    // Update heart icon
    const header = card.querySelector('.paper-header');
    if (header) {
      const oldHeart = header.querySelector('.heart-icon');
      if (oldHeart) {
        const heartClass = mark ? `heart-icon heart-${mark}` : 'heart-icon heart-empty';
        const heartFill = mark ? 'fill="currentColor"' : 'fill="none" stroke="currentColor" stroke-width="1.5"';
        const newHeart = document.createElement('span');
        newHeart.innerHTML = `<svg class="${heartClass}" viewBox="0 0 24 24" ${heartFill}><path d="M12 21.35l-1.45-1.32C5.4 15.36 2 12.28 2 8.5 2 5.42 4.42 3 7.5 3c1.74 0 3.41.81 4.5 2.09C13.09 3.81 14.76 3 16.5 3 19.58 3 22 5.42 22 8.5c0 3.78-3.4 6.86-8.55 11.54L12 21.35z"/></svg>`;
        oldHeart.replaceWith(newHeart.firstElementChild);
      }
    }

    // Update mark buttons active state
    const markGroup = card.querySelector('.mark-group');
    if (markGroup) {
      markGroup.querySelectorAll('button[data-mark]').forEach(btn => {
        btn.classList.toggle('active', btn.dataset.mark === mark);
      });
      // Update clear button
      let clearBtn = markGroup.querySelector('button:not([data-mark])');
      if (mark) {
        if (!clearBtn) {
          clearBtn = document.createElement('button');
          clearBtn.textContent = '清除';
          clearBtn.onclick = () => app.setMark(id, null);
          markGroup.appendChild(clearBtn);
        }
      } else if (clearBtn) {
        clearBtn.remove();
      }
    }
  },

  _updateCardInsightUI(id, insight, insight_review) {
    const card = document.querySelector(`.paper-card[data-id="${CSS.escape(id)}"]`);
    if (!card) return;
    const metaLeft = card.querySelector('.meta-left');
    if (!metaLeft) return;
    const sourceLink = metaLeft.querySelector('a.meta-link');
    if (!sourceLink) return;

    let html = '';
    if (insight === '__ANALYZING__') {
      html = '<span class="meta-link" style="opacity:0.6;cursor:default;">分析中...</span>';
    } else if (insight_review === '__REVIEWING__') {
      html = `<a href="/insight/${encodeURIComponent(this.paperSource(id))}/${id.split('/').map(encodeURIComponent).join('/')}" class="meta-link" style="color:var(--accent);" onclick="if (!event.metaKey && !event.ctrlKey) { event.preventDefault(); app.showInsightPage('${this.escape(id)}'); }">审查中...</a>`;
    } else if (insight) {
      html = `<a href="/insight/${encodeURIComponent(this.paperSource(id))}/${id.split('/').map(encodeURIComponent).join('/')}" class="meta-link" onclick="if (!event.metaKey && !event.ctrlKey) { event.preventDefault(); app.showInsightPage('${this.escape(id)}'); }">LLM</a>`;
    } else if (this.authenticated) {
      html = `<span class="meta-link" style="cursor:pointer" onclick="app.toggleInsight('${this.escape(id)}', event)">reqLLM</span>`;
    }

    // Remove old insight element (the one right after source link)
    let next = sourceLink.nextElementSibling;
    while (next && !next.classList.contains('meta-right')) {
      const toRemove = next;
      next = next.nextElementSibling;
      toRemove.remove();
    }

    if (html) {
      const wrapper = document.createElement('span');
      wrapper.innerHTML = html;
      sourceLink.after(wrapper.firstElementChild);
    }
  },
};
