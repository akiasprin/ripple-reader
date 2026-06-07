import { marked } from 'marked';

import { state } from './state.js';
import { utils } from './utils.js';
import { dialogs } from './dialogs.js';
import { theme } from './theme.js';
import { auth } from './auth.js';
import { bootstrap } from './bootstrap.js';
import { list } from './list.js';
import { paper } from './paper.js';
import { insight } from './insight.js';
import { insightStream } from './insight-stream.js';
import { toc } from './toc.js';
import { review } from './review.js';
import { backup } from './backup.js';
import { comments } from './comments.js';
import { highlights } from './highlights.js';
import { markdown } from './markdown.js';
import { progress } from './progress.js';
import { tags } from './tags.js';
import { orphan } from './orphan.js';
import { dates } from './dates.js';

// Expose third-party libraries globally for runtime checks in modules.
// Prism is loaded via <script src> so the autoloader plugin is available.
window.marked = marked;

// Reassemble the single global `app` object the whole UI depends on.
// Methods use `this`/`app`, so merging onto one object keeps all call sites working.
const app = Object.assign({}, state, utils, dialogs, theme, auth, bootstrap, list, paper, insight, insightStream, toc, review, backup, comments, highlights, markdown, progress, tags, orphan, dates);

// Inline onclick="app.xxx()" handlers and a few bare `app.` call sites need the global.
window.app = app;

app.init();
