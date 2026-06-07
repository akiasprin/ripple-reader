// Auto-extracted from the original monolithic app.js.
// Each module exports a slice of the global `app` object; main.js merges them.
export const toc = {
  _buildTOCFromData(toc) {
    if (!toc || toc.length < 2) return;
    // Create or reuse TOC bar button
    let bar = document.getElementById('insightTocBar');
    if (!bar) {
      bar = document.createElement('div');
      bar.id = 'insightTocBar';
      bar.className = 'toc-bar';
      bar.title = '目录';
      bar.innerHTML = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><line x1="3" y1="6" x2="21" y2="6"/><line x1="3" y1="12" x2="21" y2="12"/><line x1="3" y1="18" x2="21" y2="18"/></svg>';
      bar.addEventListener('click', () => this._toggleTOC());
      document.body.appendChild(bar);
    }
    // Create TOC panel
    let panel = document.getElementById('insightTOC');
    if (panel) panel.remove();
    panel = document.createElement('div');
    panel.id = 'insightTOC';
    panel.className = 'toc-panel';
    let html = '<div class="toc-panel-header">目录<button class="panel-collapse-btn" onclick="app._toggleTOC()" title="收起">&times;</button></div><div class="toc-panel-list">';
    toc.forEach(item => {
      const prefix = item.num ? `<span class="toc-num">${item.num}</span> ` : '';
      html += `<a class="toc-panel-item level-${item.level}" data-target="${item.id}" onclick="app._scrollToHeading('${item.id}')">${prefix}${this.escape(item.text)}</a>`;
    });
    html += '</div>';
    panel.innerHTML = html;
    document.body.appendChild(panel);

    const isOpen = localStorage.getItem('insightTOCOpen') === '1';
    if (isOpen) {
      panel.classList.add('show');
      bar.classList.add('show');
    } else {
      // Force reflow to ensure transition triggers even if bar was reused
      bar.classList.remove('show');
      void bar.offsetWidth;
      bar.classList.add('show');
    }
    // Set up scroll listener using DOM headings (already rendered)
    const headings = document.querySelectorAll('#insightPageBody h1, #insightPageBody h2, #insightPageBody h3');
    if (headings.length >= 2) this._setupTOCScrollListener(headings);
  },

  _removeTOC() {
    const panel = document.getElementById('insightTOC');
    if (panel) panel.remove();
    const bar = document.getElementById('insightTocBar');
    if (bar) {
      // Only hide the bar when leaving the insight view entirely.
      // When switching between papers, keep it visible to avoid re-animation.
      const leavingInsight = !document.getElementById('insightPage').classList.contains('open');
      if (leavingInsight) {
        bar.classList.remove('show');
      }
    }
    if (this._tocScrollHandler) {
      window.removeEventListener('scroll', this._tocScrollHandler);
      this._tocScrollHandler = null;
    }
    if (this._tocResizeHandler) {
      window.removeEventListener('resize', this._tocResizeHandler);
      this._tocResizeHandler = null;
    }
  },

  _toggleTOC() {
    const panel = document.getElementById('insightTOC');
    const bar = document.getElementById('insightTocBar');
    if (!panel) return;
    const isOpen = panel.classList.toggle('show');
    if (bar) bar.classList.toggle('show', !isOpen);
    localStorage.setItem('insightTOCOpen', isOpen ? '1' : '0');
    // Mutually exclusive with progress panel
    if (isOpen) {
      const progressPanel = document.getElementById('insightProgressPanel');
      const progressBar = document.getElementById('progressBar');
      if (progressPanel && progressPanel.classList.contains('show')) {
        this.progressCollapsed = true;
        if (progressBar) progressBar.classList.add('show');
        progressPanel.classList.remove('show');
      }
    }
  },

  _scrollToHeading(id) {
    const el = document.getElementById(id);
    if (!el) return;
    el.scrollIntoView({ behavior: 'smooth', block: 'start' });
  },

  _setupTOCScrollListener(headings) {
    if (this._tocScrollHandler) {
      window.removeEventListener('scroll', this._tocScrollHandler);
      this._tocScrollHandler = null;
    }
    if (this._tocResizeHandler) {
      window.removeEventListener('resize', this._tocResizeHandler);
      this._tocResizeHandler = null;
    }
    const offset = 80;
    const panel = document.getElementById('insightTOC');
    if (!panel) return;

    let ticking = false;
    this._tocScrollHandler = () => {
      if (ticking) return;
      ticking = true;
      requestAnimationFrame(() => {
        let activeId = null;
        for (const h of headings) {
          if (h.getBoundingClientRect().top <= offset + 15) {
            activeId = h.id;
          } else {
            break;
          }
        }
        panel.querySelectorAll('.toc-panel-item').forEach(item => {
          item.classList.toggle('active', item.dataset.target === activeId);
        });
        ticking = false;
      });
    };
    window.addEventListener('scroll', this._tocScrollHandler, { passive: true });
    this._tocScrollHandler();
  },
};
