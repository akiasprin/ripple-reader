// Auto-extracted from the original monolithic app.js.
// Each module exports a slice of the global `app` object; main.js merges them.
export const dates = {
  showDatesPage() {
    this.datesPageOpen = true;
    document.getElementById('filterBar').style.display = 'none';
    document.getElementById('stats').style.display = 'none';
    document.getElementById('list').style.display = 'none';
    document.getElementById('pagination').style.display = 'none';
    document.getElementById('insightPage').classList.remove('open');
    document.getElementById('tagsPage').classList.remove('open');
    document.getElementById('tagsPage').innerHTML = '';
    document.getElementById('datesPage').classList.add('open');
    document.getElementById('navHome').classList.remove('active');
    document.getElementById('navTags').classList.remove('active');
    document.getElementById('navDates').classList.add('active');
    window.scrollTo({ top: 0, behavior: 'instant' });
    this.hideOrphanPanel();
    this.loadDatesTree();
  },

  hideDatesPage() {
    this.datesPageOpen = false;
    this.datesSelectedDate = null;
    this.datesPapers = null;
    document.getElementById('datesPage').classList.remove('open');
    document.getElementById('datesPage').innerHTML = '';
    document.getElementById('filterBar').style.display = '';
    document.getElementById('stats').style.display = '';
    document.getElementById('list').style.display = '';
    document.getElementById('pagination').style.display = '';
    document.getElementById('navHome').classList.add('active');
    document.getElementById('navDates').classList.remove('active');
    this.hideOrphanPanel();
  },

  async loadDatesTree() {
    try {
      const res = await fetch('/api/papers/dates');
      if (!res.ok) throw new Error('加载失败');
      const data = await res.json();
      if (!this.datesPageOpen) return;
      this.datesTreeData = data;
      this.renderDatesTree();
    } catch (e) {
      document.getElementById('datesPage').innerHTML = '<div class="empty">加载日期数据失败，请刷新重试</div>';
    }
  },

  renderDatesTree() {
    const tree = this.datesTreeData || [];
    const selectedDate = this.datesSelectedDate || '';
    let html = '<div class="dates-layout">';
    // Sidebar
    html += '<div class="dates-sidebar"><div class="dates-sidebar-header">日期浏览</div>';
    if (tree.length === 0) {
      html += '<div class="dates-empty" style="padding:24px 0;font-size:13px;">暂无日期数据</div>';
    } else {
      html += '<div class="date-tree">';
      for (const year of tree) {
        const yearOpen = year.months.some(m => m.days.some(d => {
          const full = year.year + '-' + String(m.month).padStart(2, '0') + '-' + String(d.day).padStart(2, '0');
          return full === selectedDate;
        }));
        html += '<div class="date-tree-year">';
        html += `<div class="date-tree-year-label ${yearOpen ? 'open' : ''}" onclick="app.toggleDateYear(this)"><span class="arrow">&#9654;</span><span>${this.escape(year.year)}年</span><span class="count">${year.count}篇</span></div>`;
        html += `<div class="date-tree-months ${yearOpen ? 'open' : ''}">`;
        for (const month of year.months) {
          const monthOpen = month.days.some(d => {
            const full = year.year + '-' + String(month.month).padStart(2, '0') + '-' + String(d.day).padStart(2, '0');
            return full === selectedDate;
          });
          html += '<div class="date-tree-month">';
          html += `<div class="date-tree-month-label ${monthOpen ? 'open' : ''}" onclick="app.toggleDateMonth(this)"><span class="arrow">&#9654;</span><span>${this.escape(month.month)}月</span><span class="count">${month.count}篇</span></div>`;
          html += `<div class="date-tree-days ${monthOpen ? 'open' : ''}">`;
          for (const day of month.days) {
            const full = year.year + '-' + String(month.month).padStart(2, '0') + '-' + String(day.day).padStart(2, '0');
            const isActive = full === selectedDate;
            html += `<div class="date-tree-day ${isActive ? 'active' : ''}" onclick="app.selectDate('${this.escape(full)}')"><span>${this.escape(day.day)}日</span><span class="count">${day.count}篇</span></div>`;
          }
          html += '</div>'; // days
          html += '</div>'; // month
        }
        html += '</div>'; // months
        html += '</div>'; // year
      }
      html += '</div>'; // tree
    }
    html += '</div>'; // sidebar
    // Main
    html += '<div class="dates-main" id="datesMain">';
    if (selectedDate) {
      html += `<div class="dates-main-header"><h2>${this.escape(selectedDate)}</h2><span class="date-label">发表的论文</span></div>`;
      html += '<div class="dates-paper-list" id="datesPaperList"></div>';
    } else {
      html += '<div class="dates-empty">请在左侧选择日期查看论文</div>';
    }
    html += '</div>'; // main
    html += '</div>'; // layout
    document.getElementById('datesPage').innerHTML = html;
    if (selectedDate) {
      this.renderPapersByDate();
    }
  },

  toggleDateYear(el) {
    el.classList.toggle('open');
    const months = el.nextElementSibling;
    if (months) months.classList.toggle('open');
  },

  toggleDateMonth(el) {
    el.classList.toggle('open');
    const days = el.nextElementSibling;
    if (days) days.classList.toggle('open');
  },

  selectDate(date) {
    this.datesSelectedDate = date;
    this.datesPapers = null;
    this.renderDatesTree();
    this.loadPapersByDate(date);
  },

  async loadPapersByDate(date) {
    try {
      const res = await fetch('/api/papers/dates/' + encodeURIComponent(date));
      if (!res.ok) throw new Error('加载失败');
      const papers = await res.json();
      if (!this.datesPageOpen || this.datesSelectedDate !== date) return;
      this.datesPapers = papers;
      this.renderPapersByDate();
    } catch (e) {
      const list = document.getElementById('datesPaperList');
      if (list) list.innerHTML = '<div class="dates-empty">加载论文失败，请重试</div>';
    }
  },

  renderPapersByDate() {
    const list = document.getElementById('datesPaperList');
    if (!list) return;
    const papers = this.datesPapers || [];
    if (papers.length === 0) {
      list.innerHTML = '<div class="dates-empty">该日期没有论文</div>';
      return;
    }
    const html = papers.map(p => {
      const scoreClass = p.score >= 7 ? 'score-high' : p.score >= 4 ? 'score-mid' : 'score-low';
      const abstract = (p.abstract || '').substring(0, 200) + ((p.abstract || '').length > 200 ? '...' : '');
      const authors = p.authors.length > 2 ? p.authors.slice(0, 2).join(', ') + ' 等' : p.authors.join(', ');
      const markMap = { critical: '关键', supporting: '参考', marginal: '边缘' };
      const markColor = { critical: '#c75b39', supporting: '#4a7c59', marginal: '#8c7b40' };
      const markLabel = p.mark && markMap[p.mark] ? markMap[p.mark] : '';
      const markColorVal = p.mark && markColor[p.mark] ? markColor[p.mark] : '';
      const markHtml = markLabel ? `<span style="font-size:11px;font-weight:600;padding:1px 7px;border-radius:10px;background:rgba(0,0,0,0.04);color:${markColorVal};">${markLabel}</span>` : '';
      return `
        <div class="dates-paper-card" onclick="app.showInsightPage('${this.escapeJsArg(p.id)}', '${this.escapeJsArg(p.source_type || 'arxiv')}')">
          <div class="dates-paper-title">${this.escape(p.title)}</div>
          <div class="dates-paper-meta">
            <span>${this.escape(authors)}</span>
            ${markHtml}
            <span class="score-badge-mini ${scoreClass}">${p.score.toFixed(1)}</span>
          </div>
          <div class="dates-paper-abstract">${this.escape(abstract)}</div>
        </div>
      `;
    }).join('');
    list.innerHTML = html;
  },
};
