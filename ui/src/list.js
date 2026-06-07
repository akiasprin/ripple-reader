// Auto-extracted from the original monolithic app.js.
// Each module exports a slice of the global `app` object; main.js merges them.
export const list = {
  async load() {
    this.abortRequest('load');
    const ctrl = new AbortController();
    this.abortControllers.set('load', ctrl);
    try {
      let data = null;
      // Use pre-fetched initial data if available (loaded in parallel with app.js).
      // Skip if an active filter was restored from localStorage and not present
      // in the URL — the initial HTML pre-fetch only reads URL params.
      const urlHasMark = new URL(window.location.href).searchParams.has('mark');
      if (window.__initialData__ && (!this.markFilter || urlHasMark)) {
        data = await window.__initialData__;
        delete window.__initialData__;
      } else {
        delete window.__initialData__;
      }
      // Fallback to normal fetch if prefetch failed or wasn't present.
      if (!data) {
        const params = new URLSearchParams({ page: this.page, page_size: this.pageSize });
        if (this.searchQuery) params.set('search', this.searchQuery);
        if (this.markFilter) params.set('mark', this.markFilter);
        if (this.insightFilter) params.set('insight', this.insightFilter);
        if (this.checkedFilter) params.set('checked', this.checkedFilter);
        if (this.tagFilter) params.set('tag', this.tagFilter);
        if (this.sourceFilter) params.set('source', this.sourceFilter);
        if (this.sortBy !== 'published') params.set('sort', this.sortBy);
        if (this.sortOrder !== 'desc') params.set('order', this.sortOrder);
        const res = await fetch('/api/papers?' + params.toString(), { signal: ctrl.signal });
        if (!res.ok) throw new Error('list load failed: ' + res.status);
        data = await res.json();
      }
      if (data && Array.isArray(data.papers)) {
        data.papers = data.papers.map(p => this.normalizePaper(p));
      }
      this.data = data;
      this.abortControllers.delete('load');
      // Resume polling for papers still analyzing or reviewing
      if (this.data && this.data.papers) {
        for (const p of this.data.papers) {
          const isBusy = p.is_analyzing || p.is_reviewing;
          if (isBusy && !this.pollingIds.has(p.id)) {
            this.addPollingId(p.id);
          }
        }
      }
      this.renderList();
    } catch (e) {
      if (e.name === 'AbortError') return;
      this.abortControllers.delete('load');
      document.getElementById('list').innerHTML = '<div class="empty">加载失败，请刷新重试</div>';
    }
  },

  renderList() {
    const inInsightView = !!this.insightPageId;
    document.getElementById('filterBar').style.display = inInsightView ? 'none' : '';
    document.getElementById('stats').style.display = inInsightView ? 'none' : '';
    document.getElementById('list').style.display = inInsightView ? 'none' : '';
    document.getElementById('pagination').style.display = inInsightView ? 'none' : '';
    document.getElementById('tagsPage').classList.remove('open');
    if (!this.data) return;
    const { total, page, page_size, papers } = this.data;
    const list = document.getElementById('list');
    const stats = document.getElementById('stats');
    const totalPages = Math.ceil(total / page_size) || 1;

    const filterLabel = this.markFilter ? `（${this.getMarkLabel(this.markFilter)}）` : '';
    const insightLabelMap = { 'has': ' · 已筛选有 LLM 解读', 'none': ' · 已筛选无 LLM 解读' };
    const insightLabel = insightLabelMap[this.insightFilter] || '';
    const checkedLabelMap = { 'checked': ' · 已筛选已检查', 'unchecked': ' · 已筛选未检查' };
    const checkedLabel = checkedLabelMap[this.checkedFilter] || '';
    const searchLabel = this.searchQuery ? `<span style="color:var(--accent);font-weight:600;">搜索 "${this.escape(this.searchQuery)}"</span> · ` : '';
    const cancelSearch = this.searchQuery ? ` · <a href="/?" class="meta-link" onclick="event.preventDefault();app.clearSearch()">取消</a>` : '';
    const tagLabel = this.tagFilter ? `<span style="color:var(--accent);font-weight:600;">标签 "${this.escape(this.tagFilter)}"</span> · ` : '';
    const cancelTag = this.tagFilter ? ` · <a href="/?" class="meta-link" onclick="event.preventDefault();app.clearTagFilter()">取消</a>` : '';
    const sortLabels = { published: '发表日期', processed_at: '解读日期', score: '评分', checked: '检查日期' };
    const orderLabel = this.sortOrder === 'asc' ? '↑' : '↓';
    const sortLabel = (this.sortBy !== 'published' || this.sortOrder !== 'desc') ? ` · 按${sortLabels[this.sortBy] || this.sortBy}${orderLabel}` : '';
    const sourceLabels = { arxiv: 'arXiv', openreview: 'OpenReview' };
    const sourceLabel = this.sourceFilter ? ` · 来源：${sourceLabels[this.sourceFilter] || this.sourceFilter}` : '';
    const cancelSource = this.sourceFilter ? ` · <a href="/?" class="meta-link" onclick="event.preventDefault();app.clearSourceFilter()">取消</a>` : '';
    stats.innerHTML = `${searchLabel}${tagLabel}共 ${total} 篇论文${filterLabel}${insightLabel}${checkedLabel}${sortLabel}${sourceLabel} · 第 ${page}/${totalPages} 页${cancelSearch}${cancelTag}${cancelSource}`;

    // update filter bar active state
    document.querySelectorAll('#filterBar button[data-filter]').forEach(btn => {
      btn.classList.toggle('active', btn.dataset.filter === this.markFilter);
    });
    this.updateInsightFilterUI();
    this.updateCheckedFilterUI();
    this.updateSourceFilterUI();
    this.updateFilterDropdownLabel();
    document.querySelectorAll('.filter-dropdown-option').forEach(opt => {
      if (opt.dataset.sort) {
        opt.classList.toggle('active', opt.dataset.sort === (this.sortBy + ':' + this.sortOrder));
      } else if (opt.dataset.source) {
        opt.classList.toggle('active', opt.dataset.source === this.sourceFilter);
      }
    });

    if (!papers || papers.length === 0) {
      list.innerHTML = '<div class="empty">没有找到论文</div>';
      this.renderPagination(page, totalPages);
      return;
    }

    list.innerHTML = papers.map(p => this.renderCard(p)).join('');
    this.renderPagination(page, totalPages);
  },

  renderCard(p) {
    const isEditing = this.editingIds.has(p.id);
    const scoreClass = p.score >= 7 ? 'score-high' : p.score >= 4 ? 'score-mid' : 'score-low';
    const isMobile = !this.isDesktopViewport();
    const authors = isMobile
      ? (p.authors.length > 1 ? p.authors[0] + ' 等' : p.authors.join(', '))
      : (p.authors.length > 3 ? p.authors.slice(0, 3).join(', ') + ' 等' : p.authors.join(', '));
    const sourceUrl = p.source_type === 'arxiv'
      ? 'https://arxiv.org/abs/' + encodeURIComponent(p.external_id || p.id)
      : (p.source_url || '');
    const heartMark = p.mark ? `<svg class="heart-icon heart-${p.mark}" viewBox="0 0 24 24" fill="currentColor"><path d="M12 21.35l-1.45-1.32C5.4 15.36 2 12.28 2 8.5 2 5.42 4.42 3 7.5 3c1.74 0 3.41.81 4.5 2.09C13.09 3.81 14.76 3 16.5 3 19.58 3 22 5.42 22 8.5c0 3.78-3.4 6.86-8.55 11.54L12 21.35z"/></svg>` : '<svg class="heart-icon heart-empty" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M12 21.35l-1.45-1.32C5.4 15.36 2 12.28 2 8.5 2 5.42 4.42 3 7.5 3c1.74 0 3.41.81 4.5 2.09C13.09 3.81 14.76 3 16.5 3 19.58 3 22 5.42 22 8.5c0 3.78-3.4 6.86-8.55 11.54L12 21.35z"/></svg>';
    const markOptions = [
      { key: 'critical', label: '关键' },
      { key: 'supporting', label: '参考' },
      { key: 'marginal', label: '边缘' },
    ];
    const markButtons = markOptions.map(m => {
      const active = p.mark === m.key ? 'active' : '';
      return `<button class="${active}" data-mark="${m.key}" onclick="app.setMark('${this.escape(p.id)}', '${m.key}')">${m.label}</button>`;
    }).join('');
    const clearBtn = p.mark ? `<button onclick="app.setMark('${this.escape(p.id)}', null)">清除</button>` : '';

    if (isEditing) {
      return `<div class="paper-card" data-id="${this.escape(p.id)}">
        <div class="paper-header">
          ${heartMark}
          <div class="paper-title">${this.escape(p.title)}</div>
          <span class="score-badge ${scoreClass}">${p.score.toFixed(1)}</span>
        </div>
        <div class="field">
          <label>标题</label>
          <input id="f-title-${this.escape(p.id)}" value="${this.escape(p.title)}">
        </div>
        <div class="field-row">
          <div class="field">
            <label>发表日期</label>
            <input value="${this.formatDate(p.published)}" readonly>
          </div>
          <div class="field">
            <label>评分 (0-10)</label>
            <input id="f-score-${this.escape(p.id)}" type="number" step="0.1" min="0" max="10" value="${p.score}">
          </div>
          <div class="field">
            <label>类型</label>
            <input id="f-type-${this.escape(p.id)}" value="${this.escape(p.paper_type)}">
          </div>
        </div>
        <div class="field">
          <label>摘要</label>
          <textarea id="f-abstract-${this.escape(p.id)}">${this.escape(p.abstract)}</textarea>
        </div>
        <div class="field">
          <label>总结</label>
          <textarea id="f-summary-${this.escape(p.id)}">${this.escape(p.summary)}</textarea>
        </div>
        <div class="actions">
          <span class="pub-date">${this.formatDate(p.published)}</span>
          <div class="right-group">
            <button class="btn-secondary" onclick="app.cancelEdit('${this.escape(p.id)}')">取消</button>
            <button class="btn-primary" onclick="app.save('${this.escape(p.id)}')">保存</button>
          </div>
        </div>
      </div>`;
    }

    const summarySection = `
      <div class="section">
        <div class="section-label">总结</div>
        <div class="section-body">${this.renderMarkdownLite(this.escapeHtml(p.summary || ''))}</div>
      </div>`;

    return `<div class="paper-card" data-id="${this.escape(p.id)}">
      <div class="paper-header">
        ${heartMark}
        <div class="paper-title">${this.escape(p.title)}</div>
        ${p.checked_at ? '<span class="checked-badge" title="已检查">✓</span>' : ''}
        <span class="score-badge ${scoreClass}">${p.score.toFixed(1)}</span>
      </div>
      <div class="paper-meta">
        ${p.paper_type ? `<span class="type-badge">${this.escape(p.paper_type)}</span>` : ''}
        <div class="meta-left">
          <span>${this.escape(authors)}</span>
          ${sourceUrl ? `<a href="${sourceUrl}" target="_blank" class="meta-link">${p.source_type === 'arxiv' ? 'arXiv' : 'OpenReview'}</a>` : ''}
          ${p.is_analyzing ? `<span class="meta-link" style="cursor:pointer" onclick="app.cancelInsight('${this.escapeJsArg(p.id)}')">abortLLM</span>` : (p.is_reviewing ? `<span class="meta-link" style="cursor:pointer" onclick="app.cancelReviewTask('${this.escapeJsArg(p.id)}')">abortLLM</span>` : (p.has_insight ? `<a href="/insight/${encodeURIComponent(p.source_type || 'arxiv')}/${p.id.split('/').map(encodeURIComponent).join('/')}" class="meta-link" onclick="if (!event.metaKey && !event.ctrlKey) { event.preventDefault(); app.showInsightPage('${this.escape(p.id)}'); }">LLM</a>` : (this.authenticated ? `<span class="meta-link" style="cursor:pointer" onclick="app.toggleInsight('${this.escape(p.id)}', event)">reqLLM</span>` : '')))}
        </div>
        <span class="meta-right">${isMobile ? this.formatShortDate(p.published) : this.formatDate(p.published)}</span>
      </div>
      <div class="section" style="position:relative;">
        <div class="section-label">摘要</div>
        <div style="position:absolute;top:0;right:0;font-size:12px;color:var(--text3);white-space:nowrap;">${this.renderTags(p)}</div>
        <div class="section-body dark" style="padding-top:2px;">${this.renderMarkdownLite(this.escapeHtml(p.abstract || ''))}</div>
      </div>
      ${summarySection}
      <div class="actions">
        <div style="display:flex;gap:6px;align-items:center;flex-wrap:wrap;">
          ${this.authenticated ? `<div class="mark-group">${markButtons}${clearBtn}</div>` : ''}
        </div>
        ${this.authenticated ? `<div class="right-group"><button class="btn-secondary" onclick="event.stopPropagation();app.startEdit('${this.escape(p.id)}')">编辑</button><button class="btn-danger" onclick="event.stopPropagation();app.deletePaper('${this.escape(p.id)}')">删除</button></div>` : '<span style="font-size:12px;color:var(--text3)">登录后可编辑</span>'}
      </div>
    </div>`;
  },

  renderPagination(current, total) {
    const el = document.getElementById('pagination');
    if (total <= 1) { el.innerHTML = ''; return; }
    const first = `<button ${current <= 1 ? 'disabled' : ''} onclick="app.goPage(1)">首页</button>`;
    const prev = `<button ${current <= 1 ? 'disabled' : ''} onclick="app.goPage(${current - 1})">上一页</button>`;
    const next = `<button ${current >= total ? 'disabled' : ''} onclick="app.goPage(${current + 1})">下一页</button>`;
    const last = `<button ${current >= total ? 'disabled' : ''} onclick="app.goPage(${total})">末页</button>`;
    el.innerHTML = first + prev + `<span class="page-info">${current} / ${total}</span>` + next + last;
  },

  syncFromURL() {
    const url = new URL(window.location.href);
    const p = parseInt(url.searchParams.get('page'), 10);
    if (p > 0) this.page = p;
    this.searchQuery = url.searchParams.get('search') || '';
    if (url.searchParams.has('mark')) {
      this.markFilter = url.searchParams.get('mark') || '';
    } else {
      const saved = localStorage.getItem('markFilter');
      this.markFilter = saved !== null ? saved : '';
    }
    const insightVal = url.searchParams.get('insight');
    this.insightFilter = (insightVal === 'has' || insightVal === 'none') ? insightVal : '';
    const checkedVal = url.searchParams.get('checked');
    if (checkedVal === 'has') this.checkedFilter = 'checked';
    else if (checkedVal === 'none') this.checkedFilter = 'unchecked';
    else if (checkedVal === 'checked' || checkedVal === 'unchecked') this.checkedFilter = checkedVal;
    else this.checkedFilter = '';
    this.tagFilter = url.searchParams.get('tag') || '';
    this.sourceFilter = url.searchParams.get('source') || '';
    this.sortBy = url.searchParams.get('sort') || 'published';
    this.sortOrder = url.searchParams.get('order') || 'desc';
    const m = url.pathname.match(/^\/insight\/([^\/]+)\/(.+)$/);
    if (m) {
      this.insightPageSource = decodeURIComponent(m[1]);
      let pathId = decodeURIComponent(m[2]);
      if (pathId.endsWith('/edit')) {
        this.insightPageId = pathId.slice(0, -5);
        this.insightPageEditing = true;
      } else {
        this.insightPageId = pathId;
        this.insightPageEditing = false;
      }
    } else {
      this.insightPageId = null;
      this.insightPageSource = null;
      this.insightPageEditing = false;
    }
    this.tagsPageOpen = url.pathname === '/tags';
    this.datesPageOpen = url.pathname === '/dates';
    if (this.searchQuery) document.getElementById('searchInput').value = this.searchQuery;
    this.updateInsightFilterUI();
    this.updateCheckedFilterUI();
    this.updateSourceFilterUI();
    this.updateFilterDropdownLabel();
    document.querySelectorAll('.filter-dropdown-option').forEach(opt => {
      if (opt.dataset.sort) {
        opt.classList.toggle('active', opt.dataset.sort === (this.sortBy + ':' + this.sortOrder));
      } else if (opt.dataset.source) {
        opt.classList.toggle('active', opt.dataset.source === this.sourceFilter);
      }
    });
  },

  _buildURL() {
    const url = new URL(window.location.href);
    // Preserve transient params that survive across URL rebuilds
    const reply = url.searchParams.get('reply');
    if (this.insightPageId && this.insightPageSource) {
      url.pathname = '/insight/' + encodeURIComponent(this.insightPageSource) + '/' + encodeURIComponent(this.insightPageId) + (this.insightPageEditing ? '/edit' : '');
      url.search = '';
      if (reply) url.searchParams.set('reply', reply);
    } else if (this.tagsPageOpen) {
      url.pathname = '/tags';
      url.search = '';
    } else if (this.datesPageOpen) {
      url.pathname = '/dates';
      url.search = '';
    } else {
      url.pathname = '/';
      url.searchParams.set('page', this.page);
      if (this.searchQuery) url.searchParams.set('search', this.searchQuery);
      else url.searchParams.delete('search');
      if (this.markFilter) url.searchParams.set('mark', this.markFilter);
      else url.searchParams.delete('mark');
      if (this.insightFilter) url.searchParams.set('insight', this.insightFilter);
      else url.searchParams.delete('insight');
      if (this.checkedFilter) url.searchParams.set('checked', this.checkedFilter);
      else url.searchParams.delete('checked');
      if (this.tagFilter) url.searchParams.set('tag', this.tagFilter);
      else url.searchParams.delete('tag');
      if (this.sourceFilter) url.searchParams.set('source', this.sourceFilter);
      else url.searchParams.delete('source');
      if (this.sortBy !== 'published') url.searchParams.set('sort', this.sortBy);
      else url.searchParams.delete('sort');
      if (this.sortOrder !== 'desc') url.searchParams.set('order', this.sortOrder);
      else url.searchParams.delete('order');
    }
    return url;
  },

  pushURL() {
    const url = this._buildURL();
    const newUrl = url.toString();
    // Dedup: skip pushState when URL is unchanged to avoid duplicate history entries
    if (newUrl === window.location.href) return;
    window.history.pushState({}, '', url);
  },

  replaceURL() {
    const url = this._buildURL();
    const newUrl = url.toString();
    if (newUrl === window.location.href) return;
    window.history.replaceState({}, '', url);
  },

  async goPage(p) {
    this.haptic('light');
    this.page = p;
    this.pushURL();
    await this.load();
    window.scrollTo(0, 0);
  },

  async search() {
    this.page = 1;
    this.searchQuery = document.getElementById('searchInput').value.trim();
    if (this.insightPageId || this.tagsPageOpen || this.datesPageOpen) {
      this.insightPageId = null;
      this.insightPageData = null;
      this.insightPageEditing = false;
      this.tagsPageOpen = false;
      this.datesPageOpen = false;
      this.exitInsightView();
    }
    this.markFilter = '';
    localStorage.removeItem('markFilter');
    document.querySelectorAll('.filter-bar button[data-filter]').forEach(btn => {
      btn.classList.toggle('active', btn.dataset.filter === '');
    });
    this.pushURL();
    await this.load();
    this.closeSearch();
    window.scrollTo(0, 0);
  },

  async clearSearch() {
    this.page = 1;
    this.searchQuery = '';
    document.getElementById('searchInput').value = '';
    document.getElementById('searchPanel').classList.remove('open');
    this.pushURL();
    await this.load();
    window.scrollTo(0, 0);
  },

  openSearch() {
    const panel = document.getElementById('searchPanel');
    panel.classList.add('open');
    this._showSpotlightOverlay();
    this.trapFocus(panel);
  },

  closeSearch() {
    const panel = document.getElementById('searchPanel');
    const input = document.getElementById('searchInput');
    if (input) input.blur();
    if (panel) {
      delete panel._returnFocus;
      panel.classList.remove('open');
    }
    this._hideSpotlightOverlay();
    this.untrapFocus(panel);
  },

  maybeCloseSearch() {
    setTimeout(() => {
      const input = document.getElementById('searchInput');
      if (document.activeElement !== input && !input.value.trim()) {
        this.closeSearch();
      }
    }, 200);
  },

  toggleSearch() {
    this.haptic('light');
    const panel = document.getElementById('searchPanel');
    const isOpen = panel.classList.contains('open');
    if (isOpen) {
      this.closeSearch();
    } else {
      panel.classList.add('open');
      this._showSpotlightOverlay();
      this.trapFocus(panel);
      setTimeout(() => document.getElementById('searchInput').focus(), 50);
    }
  },

  async setMarkFilter(filter) {
    this.page = 1;
    this.markFilter = filter;
    localStorage.setItem('markFilter', filter);
    this.pushURL();
    await this.load();
    window.scrollTo(0, 0);
  },

  toggleFilterDropdown() {
    document.getElementById('filterDropdown').classList.toggle('open');
  },

  closeFilterDropdown() {
    document.getElementById('filterDropdown').classList.remove('open');
  },

  async onSortOptionClick(el) {
    const val = el.dataset.sort;
    const [sort, order] = val.split(':');
    if (this.sortBy === sort && this.sortOrder === order) return;
    this.page = 1;
    this.sortBy = sort;
    this.sortOrder = order;
    this.pushURL();
    await this.load();
    this.closeFilterDropdown();
    window.scrollTo(0, 0);
  },

  async onSourceOptionClick(el) {
    const val = el.dataset.source;
    this.page = 1;
    this.sourceFilter = this.sourceFilter === val ? '' : val;
    this.updateSourceFilterUI();
    this.pushURL();
    await this.load();
    this.closeFilterDropdown();
    window.scrollTo(0, 0);
  },

  updateSourceFilterUI() {
    document.querySelectorAll('.filter-dropdown-option[data-source]').forEach(opt => {
      opt.classList.toggle('active', opt.dataset.source === this.sourceFilter);
    });
  },

  updateFilterDropdownLabel() {
    const el = document.getElementById('filterDropdownLabel');
    if (!el) return;
    const sortLabels = { published: '发表日期', processed_at: '解读日期', score: '评分', checked: '检查日期' };
    const sourceLabels = { arxiv: 'arXiv', openreview: 'OpenReview' };
    const parts = [];
    if (this.sourceFilter) parts.push(sourceLabels[this.sourceFilter] || this.sourceFilter);
    if (this.sortBy !== 'published' || this.sortOrder !== 'desc') {
      const orderLabel = this.sortOrder === 'asc' ? '↑' : '↓';
      parts.push(`${sortLabels[this.sortBy] || this.sortBy}${orderLabel}`);
    }
    el.textContent = parts.length > 0 ? parts.join(' · ') : '筛选排序';
  },

  async clearSourceFilter() {
    this.page = 1;
    this.sourceFilter = '';
    this.updateSourceFilterUI();
    this.updateFilterDropdownLabel();
    this.pushURL();
    await this.load();
    window.scrollTo(0, 0);
  },

  async toggleInsightFilter() {
    this.page = 1;
    if (this.insightFilter === '') this.insightFilter = 'has';
    else if (this.insightFilter === 'has') this.insightFilter = 'none';
    else this.insightFilter = '';
    this.updateInsightFilterUI();
    this.pushURL();
    await this.load();
    window.scrollTo(0, 0);
  },

  updateInsightFilterUI() {
    const el = document.getElementById('insightFilterCheckDropdown');
    if (!el) return;
    el.dataset.state = this.insightFilter || 'off';
    const label = el.closest('.filter-dropdown-check')?.querySelector('.check-label');
    if (label) {
      const map = { '': 'LLM 解读：全部', 'has': 'LLM 解读：有', 'none': 'LLM 解读：无' };
      label.textContent = map[this.insightFilter] || map[''];
    }
  },

  async setCheckedFilter() {
    this.page = 1;
    if (this.checkedFilter === '') this.checkedFilter = 'checked';
    else if (this.checkedFilter === 'checked') this.checkedFilter = 'unchecked';
    else this.checkedFilter = '';
    this.updateCheckedFilterUI();
    this.pushURL();
    await this.load();
    window.scrollTo(0, 0);
  },

  updateCheckedFilterUI() {
    const btn = document.getElementById('checkedFilterBtn');
    if (!btn) return;
    const map = { '': '已检查', 'checked': '已检查', 'unchecked': '未检查' };
    btn.textContent = map[this.checkedFilter] || '已检查';
    btn.classList.toggle('active', !!this.checkedFilter);
    btn.title = this.checkedFilter === 'none' ? '未检查' : '已检查';
  },

  async toggleChecked(id) {
    try {
      const headers = {};
      if (this.authToken) headers['Authorization'] = 'Bearer ' + this.authToken;
      const res = await fetch('/api/papers/' + encodeURIComponent(this.paperSource(id)) + '/' + encodeURIComponent(id) + '/checked', { method: 'POST', headers });
      if (!res.ok) throw new Error('toggle checked failed');
      const data = await res.json();
      // Update local data
      if (this.insightPageData && this.insightPageData.id === id) {
        this.insightPageData.checked_at = data.checked_at;
        this.renderInsightPage();
      }
      if (this.data && this.data.papers) {
        const paper = this.data.papers.find(p => p.id === id);
        if (paper) {
          paper.checked_at = data.checked_at;
          this.renderList();
        }
      }
    } catch (e) {
      console.error('toggle checked failed', e);
      alert('检查操作失败');
    }
  },

  getMarkLabel(filter) {
    const map = { critical: '关键', supporting: '参考', marginal: '边缘', none: '未标签' };
    return map[filter] || filter;
  },
};
