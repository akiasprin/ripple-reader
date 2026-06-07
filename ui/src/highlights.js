// Auto-extracted from the original monolithic app.js.
// Each module exports a slice of the global `app` object; main.js merges them.
export const highlights = {
  showSelectionTooltip(sel) {
    // On mobile, show the bottom action bar instead of the floating tooltip
    if (!this.isDesktopViewport()) {
      const bar = document.getElementById('mobileSelectionBar');
      if (bar) bar.classList.add('show');
      return;
    }

    const tooltip = document.getElementById('selectionTooltip');
    if (!tooltip) return;

    // Measure real size while hidden
    tooltip.style.visibility = 'hidden';
    tooltip.classList.add('show');
    const tw = tooltip.offsetWidth || 90;
    const th = tooltip.offsetHeight || 32;
    tooltip.classList.remove('show');
    tooltip.style.visibility = '';

    let left, top;
    if (this.mousePos) {
      const mx = this.mousePos.x;
      const my = this.mousePos.y;
      left = mx + 12;
      top = my + 12;
      if (mx + tw + 20 > window.innerWidth) left = mx - tw - 8;
      if (my + th + 20 > window.innerHeight) top = my - th - 8;
    } else if (sel && sel.rangeCount > 0) {
      const rect = sel.getRangeAt(0).getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0) {
        left = rect.left + rect.width / 2 - tw / 2;
        top = rect.top - th - 12;
        if (left < 8) left = 8;
        if (left + tw > window.innerWidth - 8) left = window.innerWidth - tw - 8;
        if (top < 8) top = rect.bottom + 12;
        if (top + th > window.innerHeight - 8) top = window.innerHeight - th - 8;
      } else {
        left = (window.innerWidth - tw) / 2;
        top = (window.innerHeight - th) / 2;
      }
    } else {
      left = (window.innerWidth - tw) / 2;
      top = (window.innerHeight - th) / 2;
    }

    tooltip.style.left = left + 'px';
    tooltip.style.top = top + 'px';
    tooltip.classList.add('show');
  },

  hideSelectionTooltip() {
    const tooltip = document.getElementById('selectionTooltip');
    if (tooltip) tooltip.classList.remove('show');
    const bar = document.getElementById('mobileSelectionBar');
    if (bar) bar.classList.remove('show');
    delete this._touchPos;
    delete this.mousePos;
  },

  extractSelectionContext(sel) {
    const text = sel.toString();
    if (!text) return null;
    const range = sel.getRangeAt(0);
    const page = document.getElementById('insightPage');
    let before = '', after = '';
    try {
      const preRange = document.createRange();
      preRange.selectNodeContents(page);
      preRange.setEnd(range.startContainer, range.startOffset);
      before = preRange.toString().slice(-20);
    } catch (e) { /* ignore */ }
    try {
      const postRange = document.createRange();
      postRange.selectNodeContents(page);
      postRange.setStart(range.endContainer, range.endOffset);
      after = postRange.toString().slice(0, 20);
    } catch (e) { /* ignore */ }
    // Detect if selection is the page header title → summarize full article
    // When selecting a heading, clear before/after context so the AI sees
    // the full page text rather than just the surrounding snippet.
    let isHeading = false;
    try {
      const container = range.commonAncestorContainer;
      const el = container.nodeType === Node.ELEMENT_NODE ? container : container.parentElement;
      const header = el && el.closest('.insight-page-header');
      if (header) {
        isHeading = true;
        before = '';
        after = '';
      }
    } catch (e) { /* ignore */ }
    return { text, before, after, isHeading };
  },

  showImageLightbox(img) {
    const lightbox = document.getElementById('imageLightbox');
    const lightboxImg = document.getElementById('lightboxImage');
    const caption = document.getElementById('lightboxCaption');
    if (!lightbox || !lightboxImg) return;

    this._lightboxImage = img;
    const fullSrc = img.dataset.fullSrc || img.src;
    lightboxImg.src = fullSrc;
    lightboxImg.alt = img.alt || '';
    if (caption) caption.textContent = img.alt || '';

    lightbox.classList.add('open');

    this._lightboxKeyHandler = (e) => {
      if (e.key === 'Escape') this.hideImageLightbox();
    };
    document.addEventListener('keydown', this._lightboxKeyHandler);
  },

  hideImageLightbox() {
    const lightbox = document.getElementById('imageLightbox');
    if (lightbox) lightbox.classList.remove('open');
    if (this._lightboxKeyHandler) {
      document.removeEventListener('keydown', this._lightboxKeyHandler);
      delete this._lightboxKeyHandler;
    }
    delete this._lightboxImage;
  },

  onLightboxCommentClick() {
    const img = this._lightboxImage;
    this.hideImageLightbox();
    if (img) {
      this.pendingImage = img;
      this.pendingSelection = null;
      const rect = img.getBoundingClientRect();
      this.mousePos = { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
      this.showSelectionTooltip(null);
    }
  },

  updateHighlightActive() {
    document.querySelectorAll('.comment-highlight').forEach(el => {
      const cid = parseInt(el.dataset.commentId, 10);
      el.classList.toggle('active', cid === this.activeCommentId);
    });
    document.querySelectorAll('img.comment-highlight-img').forEach(el => {
      const cid = parseInt(el.dataset.commentId, 10);
      el.classList.toggle('active', cid === this.activeCommentId);
    });
  },

  scrollToHighlight(id) {
    const tryScroll = (targetId) => {
      let el = document.querySelector('.comment-highlight[data-comment-id="' + targetId + '"]');
      if (!el) el = document.querySelector('img.comment-highlight-img[data-comment-id="' + targetId + '"]');
      if (!el) return false;
      const rect = el.getBoundingClientRect();
      const targetY = window.scrollY + rect.top - window.innerHeight / 2 + rect.height / 2;
      window.scrollTo({ top: targetY, behavior: 'smooth' });
      return true;
    };
    // If comment itself has no highlight (e.g. reply), try scrolling to root parent
    let targetId = id;
    const comment = this.comments.find(c => c.id === id);
    if (comment && comment.parent_id) {
      let current = comment;
      while (current && current.parent_id) {
        const parent = this.comments.find(c => c.id === current.parent_id);
        if (parent) current = parent;
        else break;
      }
      if (current) targetId = current.id;
    }
    if (!tryScroll(targetId)) {
      let attempts = 0;
      const timer = setInterval(() => {
        attempts++;
        if (tryScroll(targetId) || attempts >= 10) clearInterval(timer);
      }, 100);
    }
  },

  renderHighlights() {
    const page = document.getElementById('insightPage');
    if (!page) return;
    // Skip rebuild if the set of highlightable quotes hasn't changed
    const quotes = (this.comments || []).filter(c => !c.parent_id).map(c => c.id + ':' + c.quote).sort().join('|');
    if (quotes && quotes === this._highlightsFingerprint) return;
    this._highlightsFingerprint = quotes;
    // Clear existing highlights first (unwrap mark elements and image highlights)
    this.unwrapHighlights(page);
    if (!this.comments || this.comments.length === 0) return;
    // For each comment, try to find and highlight the quote in body and header
    const body = document.getElementById('insightPageBody');
    const header = page.querySelector('.insight-page-header');
    this.comments.forEach(comment => {
      if (comment.parent_id) return;
      if (this.isImageQuote(comment.quote)) {
        if (body) this.highlightImage(body, comment);
        if (header) this.highlightImage(header, comment);
      } else {
        if (body) this.highlightQuote(body, comment);
        if (header) this.highlightQuote(header, comment);
      }
    });
  },

  highlightImage(container, comment) {
    const q = CSS.escape(comment.quote);
    const img = container.querySelector('img[data-full-src="' + q + '"], img[src="' + q + '"]');
    if (!img) return;
    img.classList.add('comment-highlight-img');
    img.dataset.commentId = comment.id;
    img.onclick = (e) => {
      e.stopPropagation();
      this.activeCommentId = comment.id;
      this.updateHighlightActive();
      this.renderComments();
      this.scrollToComment(comment.id);
      this.showCommentsPanel();
    };
  },

  unwrapHighlights(container) {
    const marks = container.querySelectorAll('mark.comment-highlight');
    marks.forEach(mark => {
      const parent = mark.parentNode;
      while (mark.firstChild) {
        parent.insertBefore(mark.firstChild, mark);
      }
      parent.removeChild(mark);
      parent.normalize();
    });
    container.querySelectorAll('img.comment-highlight-img').forEach(img => {
      img.classList.remove('comment-highlight-img');
      img.classList.remove('active');
      delete img.dataset.commentId;
      img.onclick = null;
    });
  },

  _findQuoteFuzzy(fullText, quote) {
    const normQuote = quote.replace(/\s+/g, ' ').trim();
    if (!normQuote) return null;
    for (let i = 0; i < fullText.length; i++) {
      let j = i;
      let qIdx = 0;
      while (j < fullText.length && qIdx < normQuote.length) {
        while (j < fullText.length && /\s/.test(fullText[j])) j++;
        while (qIdx < normQuote.length && /\s/.test(normQuote[qIdx])) qIdx++;
        if (j >= fullText.length || qIdx >= normQuote.length) break;
        if (fullText[j] !== normQuote[qIdx]) break;
        j++;
        qIdx++;
      }
      if (qIdx === normQuote.length) {
        return { start: i, end: j };
      }
    }
    return null;
  },

  highlightQuote(container, comment) {
    if (!comment.quote || comment.quote.trim().length === 0) return;
    const walker = document.createTreeWalker(
      container,
      NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT,
      (node) => {
        if (node.nodeType === Node.TEXT_NODE) return NodeFilter.FILTER_ACCEPT;
        if (node.nodeType === Node.ELEMENT_NODE && node.tagName === 'IMG') return NodeFilter.FILTER_ACCEPT;
        return NodeFilter.FILTER_SKIP;
      },
      false
    );
    const nodes = [];
    let node;
    while ((node = walker.nextNode()) !== null) {
      nodes.push(node);
    }
    // Build a map of text content with node boundaries
    let fullText = '';
    const boundaries = [];
    for (const n of nodes) {
      if (n.nodeType === Node.TEXT_NODE) {
        boundaries.push({ start: fullText.length, end: fullText.length + n.textContent.length, node: n, isImg: false });
        fullText += n.textContent;
      } else if (n.tagName === 'IMG') {
        const alt = n.alt || '';
        if (alt) {
          boundaries.push({ start: fullText.length, end: fullText.length + alt.length, node: n, isImg: true });
          fullText += alt;
        }
      }
    }
    // Try exact needle match first
    const needle = comment.before_ctx + comment.quote + comment.after_ctx;
    let idx = fullText.indexOf(needle);
    let quoteStart, quoteEnd;
    if (idx >= 0) {
      quoteStart = idx + comment.before_ctx.length;
      quoteEnd = quoteStart + comment.quote.length;
    } else {
      // Find all occurrences of the quote, then pick the one whose
      // surrounding context best matches the stored before_ctx / after_ctx.
      const positions = [];
      let pos = fullText.indexOf(comment.quote);
      while (pos !== -1) {
        positions.push(pos);
        pos = fullText.indexOf(comment.quote, pos + 1);
      }
      if (positions.length === 0) {
        // Fuzzy match: sel.toString() inserts newlines between block elements
        // while TreeWalker textContent concatenation does not. Normalize whitespace
        // and try again, mapping the match back to original fullText positions.
        const fuzzy = this._findQuoteFuzzy(fullText, comment.quote);
        if (fuzzy) {
          quoteStart = fuzzy.start;
          quoteEnd = fuzzy.end;
        } else {
          console.warn('[highlightQuote] quote not found:', comment.id, comment.quote.slice(0, 50));
          return;
        }
      } else {
        const contextScore = (p) => {
          let score = 0;
          const b = comment.before_ctx;
          const a = comment.after_ctx;
          if (b) {
            const actualBefore = fullText.slice(Math.max(0, p - b.length), p);
            for (let i = Math.min(b.length, actualBefore.length); i > 0; i--) {
              if (b.slice(-i) === actualBefore.slice(-i)) { score += i; break; }
            }
          }
          if (a) {
            const actualAfter = fullText.slice(p + comment.quote.length, p + comment.quote.length + a.length);
            for (let i = Math.min(a.length, actualAfter.length); i > 0; i--) {
              if (a.slice(0, i) === actualAfter.slice(0, i)) { score += i; break; }
            }
          }
          return score;
        };
        let bestPos = positions[0];
        let bestScore = contextScore(bestPos);
        for (let i = 1; i < positions.length; i++) {
          const s = contextScore(positions[i]);
          if (s > bestScore) { bestScore = s; bestPos = positions[i]; }
        }
        quoteStart = bestPos;
        quoteEnd = bestPos + comment.quote.length;
      }
    }
    console.log('[highlightQuote] comment', comment.id, 'quoteStart', quoteStart, 'quoteEnd', quoteEnd, 'fullTextLen', fullText.length, 'boundaries', boundaries.length);
    // Find the text nodes that contain the quote
    let startIdx = -1, endIdx = -1;
    for (let i = 0; i < boundaries.length; i++) {
      if (quoteStart >= boundaries[i].start && quoteStart < boundaries[i].end) startIdx = i;
      if (quoteEnd > boundaries[i].start && quoteEnd <= boundaries[i].end) endIdx = i;
    }
    console.log('[highlightQuote] startIdx', startIdx, 'endIdx', endIdx);
    if (startIdx < 0 || endIdx < 0) {
      console.warn('[highlightQuote] startIdx/endIdx not found:', comment.id, 'startIdx', startIdx, 'endIdx', endIdx);
      return;
    }
    // Adjust if startIdx/endIdx fall on img boundaries (alt text can't be highlighted)
    if (boundaries[startIdx].isImg) {
      let j = startIdx - 1;
      while (j >= 0 && boundaries[j].isImg) j--;
      if (j >= 0) {
        startIdx = j;
        quoteStart = boundaries[j].end;
      } else {
        j = startIdx + 1;
        while (j < boundaries.length && boundaries[j].isImg) j++;
        if (j < boundaries.length) { startIdx = j; quoteStart = boundaries[j].start; }
      }
    }
    if (boundaries[endIdx].isImg) {
      let j = endIdx + 1;
      while (j < boundaries.length && boundaries[j].isImg) j++;
      if (j < boundaries.length) {
        endIdx = j;
        quoteEnd = boundaries[j].start;
      } else {
        j = endIdx - 1;
        while (j >= 0 && boundaries[j].isImg) j--;
        if (j >= 0) { endIdx = j; quoteEnd = boundaries[j].end; }
      }
    }
    if (startIdx > endIdx || quoteStart >= quoteEnd) {
      console.warn('[highlightQuote] quote falls entirely on img alt, skipping:', comment.id);
      return;
    }
    const range = document.createRange();
    try {
      range.setStart(boundaries[startIdx].node, quoteStart - boundaries[startIdx].start);
      range.setEnd(boundaries[endIdx].node, quoteEnd - boundaries[endIdx].start);
      const mark = document.createElement('mark');
      mark.className = 'comment-highlight' + (this.activeCommentId === comment.id ? ' active' : '');
      mark.dataset.commentId = comment.id;
      mark.onclick = (e) => {
        e.stopPropagation();
        this.activeCommentId = comment.id;
        this.updateHighlightActive();
        this.renderComments();
        this.scrollToComment(comment.id);
        this.showCommentsPanel();
      };
      range.surroundContents(mark);
      console.log('[highlightQuote] surroundContents OK for comment', comment.id);
    } catch (e) {
      console.log('[highlightQuote] surroundContents failed for comment', comment.id, e.message, '→ text-node fallback');
      try {
        const marks = [];
        for (let i = startIdx; i <= endIdx; i++) {
          const info = boundaries[i];
          if (info.isImg) continue;
          const node = info.node;
          const nodeStart = Math.max(quoteStart, info.start) - info.start;
          const nodeEnd = Math.min(quoteEnd, info.end) - info.start;
          if (nodeEnd <= nodeStart) continue;
          if (!node.textContent.trim().length) continue;
          const mark = document.createElement('mark');
          mark.className = 'comment-highlight' + (this.activeCommentId === comment.id ? ' active' : '');
          mark.dataset.commentId = comment.id;
          mark.onclick = (e) => {
            e.stopPropagation();
            this.activeCommentId = comment.id;
            this.updateHighlightActive();
            this.renderComments();
            this.scrollToComment(comment.id);
            this.showCommentsPanel();
          };
          const r = document.createRange();
          r.setStart(node, nodeStart);
          r.setEnd(node, nodeEnd);
          r.surroundContents(mark);
          marks.push(mark);
        }
        if (marks.length === 0) throw new Error('no text nodes to wrap');
        // Merge adjacent marks so the highlight looks continuous
        for (let i = marks.length - 1; i > 0; i--) {
          const curr = marks[i];
          const prev = marks[i - 1];
          if (curr.previousSibling === prev) {
            while (curr.firstChild) {
              prev.appendChild(curr.firstChild);
            }
            curr.remove();
            marks.splice(i, 1);
          }
        }
      } catch (e2) {
        console.warn('[highlightQuote] Failed to highlight:', comment.id, e2);
      }
    }
  },
};
