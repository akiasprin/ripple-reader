// Auto-extracted from the original monolithic app.js.
// Each module exports a slice of the global `app` object; main.js merges them.
export const insightStream = {
  startInsightStream(id) {
    if (this.insightEventSource) {
      if (this.insightStreamId === id) return;
      this.stopInsightStream();
    }
    this.insightStreamId = id;
    this.streamingInsightText = '';
    this._resetStreamingState(true);
    const url = '/api/papers/' + encodeURIComponent(this.paperSource(id)) + '/' + encodeURIComponent(id) + '/insight-stream' + (this.authToken ? '?token=' + encodeURIComponent(this.authToken) : '');
    const es = new EventSource(url);
    this.insightEventSource = es;

    es.addEventListener('init', (e) => {
      const data = JSON.parse(e.data);
      this.streamingInsightText = data.accumulated || '';
      if (this.streamingInsightText) {
        this.updateStreamingContent();
      }
    });

    es.addEventListener('chunk', (e) => {
      const data = JSON.parse(e.data);
      this.streamingInsightText += data.text || '';
      this.updateStreamingContent();
    });

    es.addEventListener('reset', (e) => {
      this._resetStreamingState(true);
    });

    es.addEventListener('done', (e) => {
      this.stopInsightStream();
      if (this.insightPageId === id) {
        this.loadInsightPage(id);
      }
    });

    es.addEventListener('error', (e) => {
      console.error('[insight-stream] error', e);
      if (e.data) {
        try {
          const data = JSON.parse(e.data);
          if (data.message) {
            this.showToast('分析出错: ' + data.message, 'error');
          }
        } catch (_) {}
      }
      this.stopInsightStream();
    });

    es.onerror = (e) => {
      console.error('[insight-stream] connection error', e);
      this.stopInsightStream();
    };
  },

  findStableSplitPoint(text) {
    if (!text) return 0;

    // Only scan the unstable portion for code-block fences.
    // The stable portion (< _stableTextLen) was already resolved.
    const scanStart = Math.max(0, (this._stableTextLen || 0) - 1);
    const scanText = text.slice(scanStart);
    // Check for unclosed code block in the unstable portion
    let inCodeBlock = false;
    let lastFenceLine = -1;
    const lines = scanText.split('\n');
    for (let i = 0; i < lines.length; i++) {
      if (lines[i].trimStart().startsWith('```')) {
        inCodeBlock = !inCodeBlock;
        if (inCodeBlock) lastFenceLine = i;
      }
    }
    if (inCodeBlock) {
      let pos = scanStart;
      for (let i = 0; i < lastFenceLine; i++) pos += lines[i].length + 1;
      return pos;
    }

    // If text ends with \n\n+, the last block is closed → everything is stable
    if (/\n\n+$/.test(text)) return text.length;

    // Find last \n\n separator — content after it is the unstable tail
    const lastDoubleNewline = text.lastIndexOf('\n\n');
    if (lastDoubleNewline !== -1) return lastDoubleNewline + 2;

    return 0;
  },

  updateStreamingContent() {
    if (!this.streamingInsightText) return;
    const container = document.getElementById('insightStreamingContent');
    if (!container) return;
    const loadingState = document.getElementById('insightLoadingState');
    if (loadingState) loadingState.style.display = 'none';
    container.style.display = '';

    // Ensure stable/unstable child containers exist so we can mutate only
    // the unstable tail and preserve DOM (and user selection) in the stable part.
    let stableEl = container.querySelector('.stream-stable');
    let unstableEl = container.querySelector('.stream-unstable');
    if (!stableEl || !unstableEl) {
      container.innerHTML = '<div class="stream-stable"></div><div class="stream-unstable"></div>';
      stableEl = container.querySelector('.stream-stable');
      unstableEl = container.querySelector('.stream-unstable');
      this._stableTextLen = 0;
      this._stableHtml = '';
    }

    this._streamUpdatePending = true;
    if (this._streamRafId) return;

    const render = () => {
      this._streamRafId = null;
      if (!this._streamUpdatePending) return;

      const now = performance.now();
      const elapsed = now - this._streamLastRenderTime;
      const textLen = this.streamingInsightText.length;
      // Scale throttle with content length: longer text = less frequent updates
      const adaptiveThrottle = textLen > 10000 ? 300 : (textLen > 5000 ? 200 : this._streamThrottleMs);
      if (elapsed < adaptiveThrottle) {
        this._streamRafId = requestAnimationFrame(render);
        return;
      }

      this._streamUpdatePending = false;

      // Incremental rendering: only re-render the unstable tail
      const splitPoint = this.findStableSplitPoint(this.streamingInsightText);
      console.log('[sse] splitPoint:', splitPoint, 'stableLen:', this._stableTextLen, 'total:', this.streamingInsightText.length);
      if (splitPoint > this._stableTextLen) {
        const newStableText = this.streamingInsightText.slice(this._stableTextLen, splitPoint);
        console.log('[sse] stable +=', newStableText.length, 'chars');
        const newStableHtml = this.renderMarkdown(this.escapeHtml(newStableText));
        this._stableHtml += newStableHtml;
        stableEl.insertAdjacentHTML('beforeend', newStableHtml);
        this._stableTextLen = splitPoint;
      }

      const unstableText = this.streamingInsightText.slice(this._stableTextLen);
      console.log('[sse] unstable:', unstableText.length, 'chars');
      let unstableHtml;
      // Skip expensive markdown/rendering for very long streaming text;
      // plain text with newline-to-br is enough while tokens are still arriving.
      if (textLen > 50000) {
        unstableHtml = this.escapeHtml(unstableText).replace(/\n/g, '<br>');
      } else {
        unstableHtml = this.renderMarkdown(this.escapeHtml(unstableText));
      }

      console.log('[sse] stableHtml:', this._stableHtml.length, 'unstableHtml:', unstableHtml.length);
      unstableEl.innerHTML = unstableHtml;
      this._streamLastRenderTime = performance.now();

      if (this._streamUpdatePending) {
        this._streamRafId = requestAnimationFrame(render);
      }
    };

    this._streamRafId = requestAnimationFrame(render);
  },

  stopInsightStream() {
    if (this._streamRafId) {
      cancelAnimationFrame(this._streamRafId);
      this._streamRafId = null;
    }
    this._streamUpdatePending = false;
    if (this.insightEventSource) {
      this.insightEventSource.close();
      this.insightEventSource = null;
    }
    this.insightStreamId = null;
    this._resetStreamingState(false);
  },

  _resetStreamingState(clearDom) {
    this.streamingInsightText = '';
    this._stableTextLen = 0;
    this._stableHtml = '';
    this._streamUpdatePending = false;
    if (clearDom) {
      const container = document.getElementById('insightStreamingContent');
      if (container) container.innerHTML = '';
    }
  },
};
