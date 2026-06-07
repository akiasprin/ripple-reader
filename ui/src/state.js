// Auto-extracted from the original monolithic app.js.
// Each module exports a slice of the global `app` object; main.js merges them.
export const state = {
  page: 1,

  pageSize: 10,

  searchQuery: '',

  markFilter: '',

  insightFilter: '',

  checkedFilter: '',

  tagFilter: '',

  sourceFilter: '',

  sortBy: 'published',

  sortOrder: 'desc',

  data: null,

  editingIds: new Set(),

  insightEditingIds: new Set(),

  reviewEditingIds: new Set(),

  insightPageId: null,

  insightPageSource: null,

  insightPageData: null,

  pollingIds: new Set(),

  pollingTimer: null,

  progressTimer: null,

  insightPageEditing: false,

  _insightPageRendering: null,

  insightNeighbors: { prev: null, next: null },

  _prompts: null,

  previousAnalyzingItems: new Map(),

  completedItems: [],

  tagsPageOpen: false,

  datesPageOpen: false,

  datesTreeData: null,

  datesSelectedDate: null,

  datesPapers: null,

  tagsData: null,

  orphanData: null,

  uninterestingData: null,

  disinterestTags: null,

  interestTags: null,

  selectedDisinterestTags: new Set(),

  drawerType: '',

  comments: [],

  activeCommentId: null,

  pendingSelection: null,

  pendingImage: null,

  mousePos: { x: 0, y: 0 },

  lastAddedCommentId: null,

  commentsPanelOpen: false,

  progressCollapsed: false,

  insightProviders: [],

  selectedInsightProvider: null,

  abortControllers: new Map(),

  _dialogAnims: new Map(),

  _streamRafId: null,

  _streamUpdatePending: false,

  _streamLastRenderTime: 0,

  _streamThrottleMs: 150,

  _stableTextLen: 0,

  _stableHtml: '',

  _tocScrollHandler: null,
};
