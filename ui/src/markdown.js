// Auto-extracted from the original monolithic app.js.
// Each module exports a slice of the global `app` object; main.js merges them.

// --- Prettier parser on-demand loader ---
// parser plugins are loaded dynamically the first time they're needed,
// so papers without code blocks don't pay for prettier's parser payload.

const PRETTIER_PARSER_URLS = {
  babel:      '/static/vendor/prettier-parser-babel.js',
  typescript: '/static/vendor/prettier-parser-typescript.js',
  html:       '/static/vendor/prettier-parser-html.js',
  markdown:   '/static/vendor/prettier-parser-markdown.js',
  yaml:       '/static/vendor/prettier-parser-yaml.js',
  postcss:    '/static/vendor/prettier-parser-postcss.js',
  // json → babel, css/scss → postcss (see PRETTIER_PARSER_MAP)
};

const _loadedParsers = new Set();
const _parserLoadPromises = {};

/** Dynamically load prettier parser plugins. Skips already-loaded/cached parsers. */
function loadPrettierParsers(parserNames) {
  const toLoad = parserNames.filter(p => !_loadedParsers.has(p) && !_parserLoadPromises[p]);
  if (!toLoad.length) {
    const pending = parserNames.filter(p => _parserLoadPromises[p]);
    return pending.length ? Promise.all(pending.map(p => _parserLoadPromises[p])) : Promise.resolve();
  }
  const promises = toLoad.map(name => {
    const url = PRETTIER_PARSER_URLS[name];
    if (!url) return Promise.resolve();
    const p = new Promise((resolve) => {
      const s = document.createElement('script');
      s.src = url;
      s.onload = () => { _loadedParsers.add(name); resolve(); };
      s.onerror = () => { delete _parserLoadPromises[name]; resolve(); };
      document.head.appendChild(s);
    });
    _parserLoadPromises[name] = p;
    return p;
  });
  return Promise.all(promises);
}

// Map code-block language → prettier parser name.
const PRETTIER_PARSER_MAP = {
  js: 'babel', javascript: 'babel', json: 'babel',
  ts: 'typescript', typescript: 'typescript',
  css: 'postcss', scss: 'postcss',
  html: 'html', xml: 'html',
  md: 'markdown', markdown: 'markdown',
  yaml: 'yaml', yml: 'yaml',
};

export const markdown = {
  /**
   * Lightweight markdown renderer for list cards.
   * Skips heavy processing (KaTeX, image handling, Prettier, table wrapping,
   * algorithm rendering) and only applies basic inline formatting and paragraph
   * breaks.  Input is assumed to already be HTML-escaped.
   */
  renderMarkdownLite(text) {
    if (!text) return '';

    const lines = text.split('\n');
    const blocks = [];
    let currentList = null; // { type: 'ul'|'ol', items: [] }
    let paragraphLines = [];

    /** Apply inline formatting: bold, italic, code, links. */
    const applyInline = (html) => {
      html = html.replace(/\*\*([^\n*]+?)\*\*/g, '<strong>$1</strong>');
      html = html.replace(/\*([^\n*]+?)\*/g, '<em>$1</em>');
      html = html.replace(/`([^`]+)`/g, '<code>$1</code>');
      html = html.replace(/\[([^\]]+)\]\(([^)]+)\)/g, (match, txt, url) => {
        const safe = this.escapeUrlAttr(url, { allowHash: true, allowRelative: true });
        if (!safe) return match;
        return '<a href="' + safe + '" target="_blank" rel="noopener noreferrer">' + txt + '</a>';
      });
      return html;
    };

    const flushList = () => {
      if (!currentList) return;
      const tag = currentList.type === 'ul' ? 'ul' : 'ol';
      const items = currentList.items.map(item => '<li>' + applyInline(item) + '</li>').join('');
      blocks.push('<' + tag + '>' + items + '</' + tag + '>');
      currentList = null;
    };

    const flushParagraph = () => {
      if (!paragraphLines.length) return;
      const html = applyInline(paragraphLines.join('<br>'));
      blocks.push('<p>' + html + '</p>');
      paragraphLines = [];
    };

    for (const line of lines) {
      const trimmed = line.trim();
      const ulMatch = trimmed.match(/^[-*]\s+(.+)$/);
      const olMatch = trimmed.match(/^\d+\.\s+(.+)$/);

      if (ulMatch) {
        flushParagraph();
        if (currentList && currentList.type !== 'ul') flushList();
        if (!currentList) currentList = { type: 'ul', items: [] };
        currentList.items.push(ulMatch[1]);
      } else if (olMatch) {
        flushParagraph();
        if (currentList && currentList.type !== 'ol') flushList();
        if (!currentList) currentList = { type: 'ol', items: [] };
        currentList.items.push(olMatch[1]);
      } else if (trimmed === '') {
        flushParagraph();
        flushList();
      } else {
        flushList();
        paragraphLines.push(line);
      }
    }

    flushParagraph();
    flushList();
    return blocks.join('');
  },

  renderPseudocode(html) {
    return html.replace(/\\begin\{algorithm\}[\s\S]*?\\end\{algorithm\}/g, (match) => {
      if (typeof pseudocode !== 'undefined') {
        try {
          // 0. Decode HTML entities so pseudocode.js (and downstream KaTeX)
          //    sees raw characters, not &gt; / &lt; / &amp;.
          let normalized = match
            .replace(/&gt;/g, '>')
            .replace(/&lt;/g, '<')
            .replace(/&amp;/g, '&');
          // 1. Normalize algorithmicx-style mixed-case commands to uppercase
          //    since pseudocode.js only recognizes ALL-CAPS commands.
          normalized = normalized.replace(
            /\\(State|Statex|For|EndFor|If|Else|ElsIf|ElseIf|EndIf|While|EndWhile|Loop|EndLoop|Repeat|Until|Require|Ensure|Input|Output|Return|Print|Comment|Break|Continue|Function|EndFunction|Procedure|EndProcedure|Call|And|Or|Not|True|False|To|Downto|Let|Assert|Upon|EndUpon)\b/g,
            (m, cmd) => '\\' + cmd.toUpperCase()
          );
          // 2. Escape // so pseudocode.js lexer does not treat it as a line
          //    comment (commentRegex = /^(%|\/\/).*/ skips everything to EOL,
          //    which would swallow the closing } in \textit{// ...}).
          normalized = normalized.replace(/\/\//g, '  ');
          return pseudocode.renderToString(normalized, {
            mathRenderer: function(expr, displayMode) {
              return displayMode ? '$$' + expr + '$$' : '$' + expr + '$';
            }
          });
        } catch (e) {
          console.warn('[pseudocode] Render failed:', e);
        }
      }
      // Fallback: return raw LaTeX so the user at least sees the algorithm
      // source instead of HTML-escaped gibberish.
      return '<pre class="language-text"><code class="language-text">' + this.escapeHtml(match) + '</code></pre>';
    });
  },

  renderMarkdown(text, addHeadingNumbers = false, opts = {}) {
    if (!text) return '';
    const tMd0 = performance.now();
    const { deferKatex = false, deferPrettier = false } = opts;
    // Simple LRU-like cache for rendered markdown to avoid re-rendering on revisit
    const cacheKey = text.length + '|' + addHeadingNumbers + '|' + deferKatex + '|' + deferPrettier + '|' + text.slice(0, 200) + '|' + text.slice(-200);
    if (!this._mdCache) this._mdCache = new Map();
    if (this._mdCache.has(cacheKey)) {
      return this._mdCache.get(cacheKey);
    }
    // Decode all HTML entities (including &ast; -> *, &num; -> #, etc.)
    // so marked.js can process raw markdown characters.
    // Use string replacement instead of textarea.innerHTML to avoid a DOM
    // round-trip on large insight content (slow on mobile).
    if (text.includes('&')) {
      text = text.replace(/&amp;/g, '&').replace(/&lt;/g, '<').replace(/&gt;/g, '>');
    }
    // Protect page:// image references before marked parsing
    const pageImages = [];
    const dimUnit = (v) => { if (!v) return ''; if (/^\d+(\.\d+)?$/.test(v)) return v + 'px'; return v; };
    // Pre-render external images with explicit dimensions before marked parsing
    let safe = text.replace(/!\[([\s\S]*?)\]\((https?:\/\/[^)\s]+)(?:\s*=\s*(\d*(?:\.\d+)?(?:%|px)?)(?:x(\d*(?:\.\d+)?(?:%|px)?))?(?:\s+(left|right|inline|center))?)??\)/g, (match, alt, url, w, h, align) => {
      const safeUrl = this.escapeUrlAttr(url);
      if (!safeUrl) return this.escape(match);
      let style = '';
      if (w) style += `width:${dimUnit(w)};`;
      if (h) style += `height:${dimUnit(h)};`;
      if (!align) {
        const styleAttr = style ? ` style="${style}"` : '';
        return `<img alt="${this.escape(alt)}" src="${safeUrl}"${styleAttr} loading="lazy" decoding="async">`;
      }
      if (align === 'inline') {
        return `<img alt="${this.escape(alt)}" src="${safeUrl}" style="${style}vertical-align:middle;" loading="lazy" decoding="async">`;
      }
      let wrapperStyle = '';
      if (align === 'left') wrapperStyle = 'float:left;margin:0 16px 8px 0;text-align:center;';
      else if (align === 'right') wrapperStyle = 'float:right;margin:0 0 8px 16px;text-align:center;';
      else wrapperStyle = 'text-align:center;margin:16px 0;';
      if (w) wrapperStyle += `width:${dimUnit(w)};`;
      wrapperStyle += 'max-width:100%;';

      let imgOnlyStyle = '';
      if (h) imgOnlyStyle += `height:${dimUnit(h)};`;
      const imgStyle = imgOnlyStyle ? ` style="${imgOnlyStyle}max-width:100%;"` : ' style="max-width:100%;"';
      return `<div style="${wrapperStyle}"><img alt="${this.escape(alt)}" src="${safeUrl}"${imgStyle}><div style="font-size:13px;color:var(--text3);margin-top:6px;word-break:break-word;">${this.escape(alt)}</div></div>`;
    });
    // Protect page:// image references before marked parsing (supports =WxH dimension suffix and align)
    safe = safe.replace(/!\[([\s\S]*?)\]\(page:\/\/(\d+)(?:\s*=\s*(\d*(?:\.\d+)?(?:%|px)?)(?:x(\d*(?:\.\d+)?(?:%|px)?))?(?:\s+(left|right|inline|center))?)??\)/g, (match, alt, page, w, h, align) => {
      pageImages.push({ alt, page, type: 'page', width: w || undefined, height: h || undefined, align: align || 'center' });
      return `%%PAGEIMG_${pageImages.length - 1}%%`;
    });
    // Protect figure/{name} image references before marked parsing (supports =WxH dimension suffix and align)
    safe = safe.replace(/!\[([\s\S]*?)\]\(figure\/([a-zA-Z0-9_.-]+)(?:\s*=\s*(\d*(?:\.\d+)?(?:%|px)?)(?:x(\d*(?:\.\d+)?(?:%|px)?))?(?:\s+(left|right|inline|center))?)??\)/g, (match, alt, name, w, h, align) => {
      pageImages.push({ alt, name, type: 'figure', width: w || undefined, height: h || undefined, align: align || 'center' });
      return `%%PAGEIMG_${pageImages.length - 1}%%`;
    });
    // Protect table/{name} image references before marked parsing (supports =WxH dimension suffix and align)
    safe = safe.replace(/!\[([\s\S]*?)\]\(table\/([a-zA-Z0-9_.-]+)(?:\s*=\s*(\d*(?:\.\d+)?(?:%|px)?)(?:x(\d*(?:\.\d+)?(?:%|px)?))?(?:\s+(left|right|inline|center))?)??\)/g, (match, alt, name, w, h, align) => {
      pageImages.push({ alt, name, type: 'table', width: w || undefined, height: h || undefined, align: align || 'center' });
      return `%%PAGEIMG_${pageImages.length - 1}%%`;
    });
    // Convert algorithm environments to HTML via pseudocode.js before marked parsing.
    // marked.js preserves HTML tags; inline math $...$ inside will be handled by KaTeX later.
    safe = this.renderPseudocode(safe);
    // Strip markdown fenced-code wrappers (```lang) that may enclose the pseudocode HTML,
    // so marked.js does not wrap it in <pre><code>.
    safe = safe.replace(/```[a-zA-Z0-9]*\n?([\s\S]*?<div class="ps-root">[\s\S]*?<\/div>[\s\S]*?)```/g, '$1');
    // Protect LaTeX math blocks from marked parsing (underscores in math become <em>)
    const mathBlocks = [];
    // Protect escaped dollar signs so \$ is not treated as math delimiter
    const escDollarBlocks = [];
    safe = safe.replace(/\\\$/g, () => {
      escDollarBlocks.push('$');
      return `%%ESC_DOLLAR_${escDollarBlocks.length - 1}%%`;
    });
    const protect = (match) => {
      // Upgrade inline math with \tag to display math
      if (match.startsWith('$') && !match.startsWith('$$') && match.includes('\\tag')) {
        match = '$$' + match.slice(1, -1) + '$$';
      }
      mathBlocks.push(match);
      return `%%MATH_${mathBlocks.length - 1}%%`;
    };
    // Upgrade standalone inline math (on its own line) to display math for centering
    safe = safe.replace(/(^|\n)\s*\$([^$\n]+?)\$\s*(?=\n|$)/g, (match, prefix, content) => {
      return prefix + '$$' + content.trim() + '$$';
    });
    // Display math $$...$$ first (greedy multi-line)
    safe = safe.replace(/\$\$[\s\S]*?\$\$/g, protect);
    // Inline math $...$ — avoid matching prices/plain text by checking content
    safe = safe.replace(/\$([^$\n]+?)\$/g, (match, content) => {
      const trimmed = content.trim();
      // Skip price-like content (pure digits/commas/dots)
      if (/^[0-9,.\s]+$/.test(trimmed)) return match;
      // Skip if content has spaces at edges (like "$ XX $")
      if (content !== trimmed) return match;
      return protect(match);
    });
    // Protect **bold** and *italic* from marked.js over-matching across lines
    const strongBlocks = [];
    safe = safe.replace(/\*\*([^\n*]+?)\*\*/g, (match, content) => {
      strongBlocks.push(content);
      return `%%STRONG_${strongBlocks.length - 1}%%`;
    });
    const emBlocks = [];
    safe = safe.replace(/\*([^\n*]+?)\*/g, (match, content) => {
      emBlocks.push(content);
      return `%%EM_${emBlocks.length - 1}%%`;
    });
    // Parse markdown on safe text (force sync for marked.js v15+)
    const tParse0 = performance.now();
    let html = marked.parse(safe, { async: false });
    console.log('[perf]     marked.parse:', (performance.now() - tParse0).toFixed(0), 'ms, input:', safe.length, 'chars, output:', html.length, 'chars');
    // Fix: marked.js autolinks greedily swallow trailing CJK chars/punctuation
    // (e.g. "https://example.com。" becomes one link including the period)
    html = html.replace(
      /<a([^>]*)>(https?:\/\/[^\s<]+?)([\u4e00-\u9fff\u3000-\u303f\uff00-\uffef][^<]*?)<\/a>/gi,
      (match, attrs, url, rest) => {
        const fixedAttrs = attrs.replace(/\bhref\s*=\s*["'][^"']*["']/i, `href="${url}"`);
        return `<a${fixedAttrs}>${url}</a>${rest}`;
      }
    );
    // Restore bold/italic placeholders (run before restoring math)
    strongBlocks.forEach((content, i) => {
      html = html.replace(`%%STRONG_${i}%%`, `<strong>${content}</strong>`);
    });
    emBlocks.forEach((content, i) => {
      html = html.replace(`%%EM_${i}%%`, `<em>${content}</em>`);
    });
    // Restore math blocks. When deferKatex is true, generate .math-deferred
    // elements AFTER markdown processing so the HTML cannot be corrupted by
    // bold/italic regexes or markdown parsers that would misinterpret * and _.
    mathBlocks.forEach((block, i) => {
      let rendered = block;
      if (deferKatex) {
        const isDisplay = block.startsWith('$$');
        const latex = isDisplay ? block.slice(2, -2).trim() : block.slice(1, -1);
        const tag = isDisplay ? 'div' : 'span';
        rendered = `<${tag} class="math-deferred" data-latex="${this.escape(latex)}" data-display="${isDisplay}">${this.escape(block)}</${tag}>`;
      } else if (typeof katex !== 'undefined') {
        try {
          if (block.startsWith('$$') && block.endsWith('$$')) {
            const latex = block.slice(2, -2).trim();
            rendered = katex.renderToString(latex, { displayMode: true, throwOnError: false, output: 'html' });
          } else if (block.startsWith('$') && block.endsWith('$')) {
            const latex = block.slice(1, -1);
            rendered = katex.renderToString(latex, { displayMode: false, throwOnError: false, output: 'html' });
            if (latex.length > 15) {
              rendered = rendered.replace(/class="katex"/g, 'class="katex katex-wide"');
            }
          }
        } catch (e) {
          console.warn('[katex] Render failed for:', block, e);
        }
      }
      html = html.replace(`%%MATH_${i}%%`, rendered);
    });
    // Restore escaped dollar signs
    escDollarBlocks.forEach((content, i) => {
      html = html.replace(`%%ESC_DOLLAR_${i}%%`, content);
    });
    // Restore page/figure images
    pageImages.forEach((img, i) => {
      let src, thumb;
      if (this.insightPageId && this.insightPageSource) {
        const base = '/api/papers/' + encodeURIComponent(this.insightPageSource) + '/' + encodeURIComponent(this.insightPageId);
        if (img.type === 'figure') {
          src = base + '/figure/' + img.name;
          thumb = src + '/thumb';
        } else if (img.type === 'table') {
          src = base + '/table/' + img.name;
          thumb = src + '/thumb';
        } else {
          src = base + '/page/' + img.page;
          thumb = src + '/thumb';
        }
      } else {
        src = img.type === 'figure' ? 'figure://' + img.name : (img.type === 'table' ? 'table://' + img.name : 'page://' + img.page);
        thumb = src;
      }
      const placeholder = `%%PAGEIMG_${i}%%`;
      const altText = this.escape(img.alt);
      const dimUnit = (v) => { if (!v) return ''; if (/^\d+(\.\d+)?$/.test(v)) return v + 'px'; return v; };
      // Numeric pixel value for HTML width/height attributes (CLS prevention)
      const dimPx = (v) => { if (!v) return ''; const m = v.match(/^(\d+(?:\.\d+)?)(px)?$/); return m ? Math.round(parseFloat(m[1])) : ''; };
      let style = '';
      if (img.width) style += `width:${dimUnit(img.width)};`;
      if (img.height) style += `height:${dimUnit(img.height)};`;
      // Use full image directly; progressive loading handles dimension swap
      let imgAttrs = 'class="progressive-img" src="' + src + '"';
      const wPx = dimPx(img.width), hPx = dimPx(img.height);
      if (wPx) imgAttrs += ' width="' + wPx + '"';
      if (hPx) imgAttrs += ' height="' + hPx + '"';

      let replacement;
      const align = img.align || 'center';
      if (align === 'inline') {
        const imgStyle = style ? ' style="' + style + 'vertical-align:middle;"' : ' style="vertical-align:middle;"';
        replacement = '<img alt="' + altText + '" ' + imgAttrs + imgStyle + ' loading="lazy" decoding="async">';
      } else if (align === 'left' || align === 'right') {
        let wrapperStyle = 'float:' + align + ';text-align:center;';
        if (align === 'left') wrapperStyle += 'margin:0 16px 8px 0;';
        else wrapperStyle += 'margin:0 0 8px 16px;';
        if (img.width) wrapperStyle += 'width:' + dimUnit(img.width) + ';';
        wrapperStyle += 'max-width:100%;';

        let imgOnlyStyle = '';
        if (img.height) imgOnlyStyle += 'height:' + dimUnit(img.height) + ';';
        const imgStyle = imgOnlyStyle ? ' style="' + imgOnlyStyle + 'max-width:100%;"' : ' style="max-width:100%;"';

        replacement = '<div style="' + wrapperStyle + '">'
          + '<img alt="' + altText + '" ' + imgAttrs + imgStyle + ' loading="lazy" decoding="async">'
          + '<div style="font-size:13px;color:var(--text3);margin-top:6px;word-break:break-word;">' + altText + '</div>'
          + '</div>';
      } else {
        const imgStyle = style ? ' style="' + style + '"' : '';
        const wrapperStyle = 'text-align:center;margin:16px 0;';
        replacement = '<div style="' + wrapperStyle + '">'
          + '<img alt="' + altText + '" ' + imgAttrs + imgStyle + ' loading="lazy" decoding="async">'
          + '<div style="font-size:13px;color:var(--text3);margin-top:6px;">' + altText + '</div>'
          + '</div>';
      }

      let idx = html.indexOf(placeholder);
      if (idx < 0) return;

      // Unwrap floated images from <p> tags so text can actually wrap around them
      if (align === 'left' || align === 'right') {
        let pStart = html.lastIndexOf('<p>', idx);
        let pEnd = html.indexOf('</p>', idx);
        if (pStart >= 0 && pEnd > idx) {
          const beforeInP = html.slice(pStart + 3, idx).trim();
          const afterInP = html.slice(idx + placeholder.length, pEnd).trim();
          if (!beforeInP && !afterInP) {
            html = html.slice(0, pStart) + replacement + html.slice(pEnd + 4);
            return;
          }
        }
      }

      const before = html.slice(0, idx);
      const after = html.slice(idx + placeholder.length);
      html = before + replacement + after;
    });
    // Clean up empty bold/italic tags left by removed inline images
    html = html.replace(/<(strong|em|b|i)>\s*<\/\1>/g, '');
    // Collapse multiple spaces, but protect pre/code blocks
    html = html.replace(/(<pre><code[\s\S]*?<\/code><\/pre>)|  +/g, (match, codeBlock) => {
      return codeBlock || ' ';
    });
    // Remove <br> right after image blocks
    html = html.replace(/(<\/div>)\s*<br\s*\/?>/g, '$1');
    // Remove empty paragraphs (including those with only whitespace)
    html = html.replace(/<p>\s*<\/p>/g, '');
    // Format code blocks with Prettier (only for known real languages, skip pseudocode)
    if (!deferPrettier && typeof prettier !== 'undefined' && window.prettierPlugins) {
      const plugins = Object.values(window.prettierPlugins);
      html = html.replace(/<pre><code(?: class="language-([^"]*)")?>([\s\S]*?)<\/code><\/pre>/g, (match, lang, code) => {
        if (!lang || !PRETTIER_PARSER_MAP[lang]) {
          // Add default class so CSS styling (background/border) still applies even without syntax highlight
          const safeLang = lang || 'text';
          return `<pre class="language-${safeLang}"><code class="language-${safeLang}">${code}</code></pre>`;
        }
        const parser = PRETTIER_PARSER_MAP[lang];
        try {
          const decoded = code.replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&amp;/g, '&');
          const formatted = prettier.format(decoded, { parser, plugins, tabWidth: 2, useTabs: false });
          const escaped = formatted.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
          return `<pre class="language-${lang}"><code class="language-${lang}">${escaped}</code></pre>`;
        } catch (e) {
          return match;
        }
      });
    } else if (deferPrettier) {
      // Defer prettier formatting: mark supported code blocks for later async processing
      html = html.replace(/<pre><code(?: class="language-([^"]*)")?>([\s\S]*?)<\/code><\/pre>/g, (match, lang, code) => {
        const safeLang = lang || 'text';
        const deferAttr = PRETTIER_PARSER_MAP[lang] ? ' data-defer-prettier="1"' : '';
        return `<pre class="language-${safeLang}"><code class="language-${safeLang}"${deferAttr}>${code}</code></pre>`;
      });
    } else {
      // Prettier not available: still ensure every code block has a language-* class
      // so Prism can render it. Fenced blocks without a language tag default to "text".
      html = html.replace(/<pre><code(?: class="language-([^"]*)")?>([\s\S]*?)<\/code><\/pre>/g, (match, lang, code) => {
        const safeLang = lang || 'text';
        return `<pre class="language-${safeLang}"><code class="language-${safeLang}">${code}</code></pre>`;
      });
    }
    // Wrap tables in scrollable containers via regex to avoid DOM round-trip.
    // On mobile, creating a detached element + innerHTML + querySelectorAll +
    // serializing back is very expensive for large insight content.
    if (html.includes('<table>')) {
      html = html.replace(/<table([^>]*)>([\s\S]*?)<\/table>/gi, '<div class="table-scroll"><table$1>$2</table></div>');
    }
    // Add heading numbers via regex instead of DOM parsing.
    if (addHeadingNumbers) {
      const counts = [0, 0, 0, 0];
      let minLevel = 4;
      html.replace(/<h([1-4])\b[^>]*?>/gi, (match, level) => {
        minLevel = Math.min(minLevel, parseInt(level) - 1);
        return match;
      });
      html = html.replace(/<h([1-4])\b([^>]*?)>/gi, (match, level, attrs) => {
        const lvl = parseInt(level) - 1;
        counts[lvl]++;
        for (let i = lvl + 1; i < 4; i++) counts[i] = 0;
        const num = counts.slice(minLevel, lvl + 1).join('.');
        if (/data-heading-num\s*=/.test(attrs)) return match;
        return `<h${level}${attrs} data-heading-num="${num}">`;
      });
    }
    html = this.sanitizeHtml(html);
    if (this._mdCache.size >= 30) {
      const firstKey = this._mdCache.keys().next().value;
      this._mdCache.delete(firstKey);
    }
    this._mdCache.set(cacheKey, html);
    console.log('[perf]   renderMarkdown total:', (performance.now() - tMd0).toFixed(0), 'ms');
    return html;
  },

  renderDeferredMath(container, onComplete) {
    if (!container) container = document.getElementById('insightPage');
    if (!container || typeof katex === 'undefined') return;
    const items = Array.from(container.querySelectorAll('.math-deferred'));
    if (!items.length) {
      if (onComplete) onComplete();
      return;
    }
    const processAll = () => {
      for (let i = 0; i < items.length; i++) {
        const el = items[i];
        if (el.dataset.rendered) continue;
        const latex = el.dataset.latex;
        if (!latex) continue;
        const display = el.dataset.display === 'true';
        try {
          const rendered = katex.renderToString(latex, { displayMode: display, throwOnError: false, strict: false, output: 'html' });
          el.innerHTML = rendered;
          el.dataset.rendered = '1';
          if (!display && latex.length > 15) {
            el.querySelectorAll('.katex').forEach(k => k.classList.add('katex-wide'));
          }
        } catch (e) {
          console.warn('[katex] Deferred render failed:', latex, e);
          // On render failure, show the raw LaTeX as fallback instead of broken HTML
          el.textContent = '$' + latex + '$';
          el.dataset.rendered = '1';
        }
      }
      if (onComplete) onComplete();
    };
    // Use requestIdleCallback with a 2s timeout so deferred math never gets
    // stuck as raw LaTeX when the page stays busy (loading images, streaming).
    if (typeof requestIdleCallback !== 'undefined') {
      requestIdleCallback(processAll, { timeout: 2000 });
    } else {
      setTimeout(processAll, 0);
    }
  },

  renderDeferredPrettier(container) {
    if (!container) container = document.getElementById('insightPage');
    if (!container) return;

    // Collect code blocks marked for deferred prettier formatting.
    const codes = Array.from(container.querySelectorAll('pre code[data-defer-prettier]'));
    if (!codes.length) {
      // No deferred blocks — still run Prism highlighting for any code blocks.
      if (typeof Prism !== 'undefined') {
        container.querySelectorAll('pre code[class^="language-"]').forEach(block => {
          if (!block.dataset.prismHighlight) {
            Prism.highlightElement(block);
            block.dataset.prismHighlight = '1';
          }
        });
      }
      return;
    }

    // Determine which parsers are actually needed.
    const needed = new Set();
    for (const el of codes) {
      const cls = el.className.match(/language-(\S+)/);
      if (cls && PRETTIER_PARSER_MAP[cls[1]]) needed.add(PRETTIER_PARSER_MAP[cls[1]]);
    }
    if (!needed.size) return;

    // Prettier core must be loaded (via <script defer>).
    if (typeof prettier === 'undefined') return;

    // Load needed parsers on demand, then format.
    loadPrettierParsers([...needed]).then(() => {
      const plugins = Object.values(window.prettierPlugins);
      let idx = 0;
      const highlightAll = () => {
        if (typeof Prism !== 'undefined') {
          container.querySelectorAll('pre code[class^="language-"]').forEach(block => {
            if (!block.dataset.prismHighlight) {
              Prism.highlightElement(block);
              block.dataset.prismHighlight = '1';
            }
          });
        }
      };
      const processBatch = () => {
        const end = Math.min(idx + 1, codes.length);
        for (; idx < end; idx++) {
          const el = codes[idx];
          const cls = el.className.match(/language-(\S+)/);
          if (!cls) continue;
          const lang = cls[1];
          const parser = PRETTIER_PARSER_MAP[lang];
          if (!parser) continue;
          try {
            const raw = el.textContent;
            const formatted = prettier.format(raw, { parser, plugins, tabWidth: 2, useTabs: false });
            el.textContent = formatted;
            el.removeAttribute('data-defer-prettier');
            if (typeof Prism !== 'undefined') {
              Prism.highlightElement(el);
              el.dataset.prismHighlight = '1';
            }
          } catch (e) {
            // skip on error
          }
        }
        if (idx < codes.length) {
          if (typeof requestIdleCallback !== 'undefined') {
            requestIdleCallback(processBatch);
          } else {
            setTimeout(processBatch, 50);
          }
        } else {
          highlightAll();
        }
      };
      if (typeof requestIdleCallback !== 'undefined') {
        requestIdleCallback(processBatch);
      } else {
        setTimeout(processBatch, 50);
      }
    });
  },
};
