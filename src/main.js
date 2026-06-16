import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';

// ============================================
// 应用状态
// ============================================
const PAGE_SIZE = 999999;

const state = {
  videos: [],
  tags: [],
  currentView: 'all',
  previousView: 'all',
  selectedTagIds: [],
  viewMode: 'grid',
  sortBy: 'updated',
  ratingFilter: 0,
  searchQuery: '',
  selectedVideoId: null,
  currentPage: 0,
  totalCount: 0,
  hasMore: false,
  loading: false,
  seriesList: [],
  currentSeries: '',
  lastClickedSeries: null,
};

// 拖拽状态
const dragState = {
  dragging: false,
  dragItem: null,
  startY: 0,
  startIndex: -1,
  placeholder: null,
};

// ============================================
// 工具函数
// ============================================
function escapeHtml(str) {
  const div = document.createElement('div');
  div.textContent = str;
  return div.innerHTML;
}

function formatSize(bytes) {
  if (bytes === 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.floor(Math.log(bytes) / Math.log(1024));
  return (bytes / Math.pow(1024, i)).toFixed(1) + ' ' + units[i];
}

function renderRatingStars(rating, interactive = false) {
  const full = Math.floor(rating / 2);
  const half = (rating / 2) % 1 >= 0.5 ? 1 : 0;
  let html = '';
  for (let i = 0; i < 5; i++) {
    if (i < full) {
      html += '<span class="star active">★</span>';
    } else if (i === full && half) {
      html += '<span class="star active">★</span>';
    } else {
      html += '<span class="star">☆</span>';
    }
  }
  return html;
}

function renderRatingInput(rating) {
  let html = '';
  for (let i = 1; i <= 10; i++) {
    const active = i <= rating ? 'active' : '';
    html += `<button class="star-btn ${active}" data-rating="${i}">${i <= rating ? '★' : '☆'}</button>`;
  }
  html += `<span class="rating-value">${rating.toFixed(1)}</span>`;
  return html;
}

// 自定义确认对话框（Tauri WebView 的 confirm 不可靠）
function showConfirm(title, message) {
  return new Promise(function(resolve) {
    var modal = document.getElementById('confirm-modal');
    document.getElementById('confirm-title').textContent = title;
    document.getElementById('confirm-message').textContent = message;
    modal.classList.remove('hidden');

    var okBtn = document.getElementById('confirm-ok');
    var cancelBtn = document.getElementById('confirm-cancel');

    function cleanup() {
      modal.classList.add('hidden');
      okBtn.onclick = null;
      cancelBtn.onclick = null;
    }

    okBtn.onclick = function() { cleanup(); resolve(true); };
    cancelBtn.onclick = function() { cleanup(); resolve(false); };
    modal.querySelector('.modal-overlay').onclick = function() { cleanup(); resolve(false); };
  });
}

function showToast(message, type = 'success') {
  const existing = document.querySelector('.toast');
  if (existing) existing.remove();

  const toast = document.createElement('div');
  toast.className = `toast ${type}`;
  toast.textContent = message;
  document.body.appendChild(toast);

  setTimeout(() => toast.remove(), 3000);
}

// ============================================
// Tauri API 调用
// ============================================
async function fetchVideos(append = false) {
  if (state.loading) return;
  state.loading = true;

  // 默认隐藏面包屑导航
  document.getElementById('series-breadcrumb').classList.add('hidden');

  try {
    const page = append ? state.currentPage : 0;
    let result;

    switch (state.currentView) {
      case 'favorites':
        // 收藏夹 + 标签组合筛选
        if (state.selectedTagIds.length > 0) {
          result = await invoke('get_videos_by_tags', { tagIds: state.selectedTagIds, videoType: null, favoritesOnly: true, page, pageSize: PAGE_SIZE });
        } else {
          result = await invoke('get_favorites', { page, pageSize: PAGE_SIZE });
        }
        break;
      case 'series':
        // 剧集 + 标签组合筛选
        if (state.selectedTagIds.length > 0) {
          result = await invoke('get_videos_by_tags', { tagIds: state.selectedTagIds, videoType: 'episode', page, pageSize: PAGE_SIZE });
        } else {
          // 剧集视图：显示剧集概览，不是单个视频
          var seriesList = await invoke('get_series_overview');
          if (state.searchQuery) {
            var q = state.searchQuery.toLowerCase();
            seriesList = (seriesList || []).filter(function(s) {
              return s.name && s.name.toLowerCase().indexOf(q) !== -1;
            });
          }
          state.videos = [];
          var filtered = seriesList || [];

          // 评分过滤
          if (state.ratingFilter > 0) {
            filtered = filtered.filter(function(s) { return (s.rating || 0) >= state.ratingFilter; });
          }

          // 排序
          switch (state.sortBy) {
            case 'name':
              filtered.sort(function(a, b) { return (a.name || '').localeCompare(b.name || ''); });
              break;
            case 'size':
              filtered.sort(function(a, b) { return (b.total_size || 0) - (a.total_size || 0); });
              break;
            case 'rating':
              filtered.sort(function(a, b) { return (b.rating || 0) - (a.rating || 0); });
              break;
            case 'updated':
            default:
              filtered.sort(function(a, b) { return new Date(b.updated_at || 0) - new Date(a.updated_at || 0); });
          }

          // 将上次点击的剧集移到最前面
          if (state.lastClickedSeries) {
            var clickedIdx = -1;
            for (var ci = 0; ci < filtered.length; ci++) {
              if (filtered[ci].name === state.lastClickedSeries) { clickedIdx = ci; break; }
            }
            if (clickedIdx > 0) {
              var clickedItem = filtered.splice(clickedIdx, 1)[0];
              filtered.unshift(clickedItem);
            }
          }

          state.seriesList = filtered;
          state.totalCount = state.seriesList.length;
          state.hasMore = false;
          applyFilters();
          renderSeriesOverview();
          updateStats();
          state.loading = false;
          return;
        }
        break;
      case 'series-episodes':
        // 显示某个剧集的剧集列表
        var episodes = await invoke('get_series_episodes', { series: state.currentSeries });
        if (state.searchQuery) {
          var q = state.searchQuery.toLowerCase();
          episodes = (episodes || []).filter(function(ep) {
            var v = ep.video || ep;
            var title = (v.title || '').toLowerCase();
            var fileName = (v.file_name || '').toLowerCase();
            var seriesName = (v.series_name || '').toLowerCase();
            return title.indexOf(q) !== -1 || fileName.indexOf(q) !== -1 || seriesName.indexOf(q) !== -1;
          });
        }
        state.videos = episodes || [];
        state.totalCount = state.videos.length;
        state.hasMore = false;
        applyFilters();
        renderVideos();
        updateStats();

        // 显示面包屑导航
        var breadcrumb = document.getElementById('series-breadcrumb');
        var seriesDisplayName = state.currentSeries && state.currentSeries.trim() !== '' ? state.currentSeries : '未命名剧集';
        document.getElementById('breadcrumb-series-name').textContent = seriesDisplayName;
        document.getElementById('breadcrumb-episode-count').textContent = state.videos.length + ' 集';
        breadcrumb.classList.remove('hidden');

        state.loading = false;
        return;
      default:
        // 电影（默认）+ 标签组合筛选
        if (state.selectedTagIds.length > 0) {
          result = await invoke('get_videos_by_tags', { tagIds: state.selectedTagIds, videoType: 'movie', page, pageSize: PAGE_SIZE });
        } else if (state.searchQuery) {
          result = await invoke('search_videos', { query: state.searchQuery, page, pageSize: PAGE_SIZE });
        } else {
          result = await invoke('get_movies', { page, pageSize: PAGE_SIZE });
        }
    }

    var items = result.items || [];
    state.totalCount = result.total || 0;
    state.hasMore = result.has_more || false;
    state.currentPage = result.page || 0;

    if (append) {
      state.videos = [...state.videos, ...items];
    } else {
      state.videos = items;
    }

    applyFilters();
    renderVideos();
    updateStats();
  } catch (e) {
    console.error('获取视频失败:', e);
    showToast('获取视频失败: ' + e, 'error');
  } finally {
    state.loading = false;
  }
}

async function fetchTags() {
  try {
    state.tags = await invoke('get_tags') || [];
    renderTags();
  } catch (e) {
    console.error('获取标签失败:', e);
  }
}

async function fetchHistory() {
  try {
    const history = await invoke('get_history', { limit: 50 });
    renderHistory(history || []);
  } catch (e) {
    console.error('获取历史失败:', e);
  }
}

async function fetchStats() {
  try {
    const stats = await invoke('get_stats');
    updateStatsDisplay(stats);
  } catch (e) {
    console.error('获取统计失败:', e);
  }
}

// ============================================
// 渲染视频列表
// ============================================
function applyFilters() {
  let videos = [...state.videos];

  // 获取视频属性（兼容 flatten 和嵌套结构）
  const getVideo = (item) => item.video || item;

  // 评分过滤
  if (state.ratingFilter > 0) {
    videos = videos.filter(v => getVideo(v).rating >= state.ratingFilter);
  }

  // 排序
  switch (state.sortBy) {
    case 'name':
      videos.sort((a, b) => (getVideo(a).title || '').localeCompare(getVideo(b).title || ''));
      break;
    case 'size':
      videos.sort((a, b) => (getVideo(b).file_size || 0) - (getVideo(a).file_size || 0));
      break;
    case 'rating':
      videos.sort((a, b) => (getVideo(b).rating || 0) - (getVideo(a).rating || 0));
      break;
    case 'updated':
    default:
      videos.sort((a, b) => new Date(getVideo(b).updated_at || 0) - new Date(getVideo(a).updated_at || 0));
  }

  state.videos = videos;
}

function renderVideos() {
  const container = document.getElementById('video-container');
  const emptyState = document.getElementById('empty-state');

  if (state.videos.length === 0) {
    container.innerHTML = '';
    emptyState.classList.remove('hidden');
    return;
  }

  emptyState.classList.add('hidden');

  let html = state.videos.map(v => renderVideoCard(v)).join('');

  // 加载更多按钮
  if (state.hasMore) {
    html += `<div class="load-more-container" style="grid-column:1/-1;text-align:center;padding:20px;">
      <button class="btn btn-primary" id="btn-load-more">
        加载更多 (${state.videos.length}/${state.totalCount})
      </button>
    </div>`;
  } else if (state.totalCount > PAGE_SIZE) {
    html += `<div style="grid-column:1/-1;text-align:center;padding:20px;color:var(--text-muted);font-size:13px;">
      已加载全部 ${state.totalCount} 个视频
    </div>`;
  }

  container.innerHTML = html;

  // 绑定点击事件
  container.querySelectorAll('.video-card').forEach(card => {
    card.addEventListener('click', () => {
      const id = parseInt(card.dataset.id);
      // 将点击的卡片移到最前面
      container.insertBefore(card, container.firstChild);
      // 同步更新 state.videos 数组顺序
      const idx = state.videos.findIndex(v => (v.video || v).id === id);
      if (idx > 0) {
        var item = state.videos.splice(idx, 1)[0];
        state.videos.unshift(item);
      }
      openVideoDetail(id);
    });
  });

  // 加载更多按钮事件
  const loadMoreBtn = document.getElementById('btn-load-more');
  if (loadMoreBtn) {
    loadMoreBtn.addEventListener('click', () => {
      state.currentPage++;
      fetchVideos(true);
    });
  }
}

// 渲染剧集概览（剧集列表页面）
function renderSeriesOverview() {
  var container = document.getElementById('video-container');
  var emptyState = document.getElementById('empty-state');

  if (!state.seriesList || state.seriesList.length === 0) {
    container.innerHTML = '';
    emptyState.classList.remove('hidden');
    return;
  }

  emptyState.classList.add('hidden');
  var html = '';
  for (var i = 0; i < state.seriesList.length; i++) {
    var s = state.seriesList[i];
    var displayName = s.name && s.name.trim() !== '' ? s.name : '未命名剧集';
    var pct = Math.round(s.progress * 100);
    var ratingStr = s.rating > 0 ? s.rating.toFixed(1) : '-';
    var sizeStr = formatSize(s.total_size);

    html += '<div class="video-card" data-series="' + escapeHtml(s.name) + '" style="border-left:3px solid ' + (pct === 100 ? '#10b981' : '#6366f1') + ';">' +
      '<div class="video-card-title">' + escapeHtml(displayName) + '</div>' +
      '<div class="video-card-meta">' + s.watched_episodes + '/' + s.total_episodes + ' 集  ·  ' + sizeStr + '  ·  ★ ' + ratingStr + '</div>' +
      '<div style="margin-top:6px;background:#252540;border-radius:4px;height:6px;overflow:hidden;">' +
        '<div style="height:100%;width:' + pct + '%;background:' + (pct === 100 ? '#10b981' : '#6366f1') + ';border-radius:4px;transition:width 0.3s;"></div>' +
      '</div>' +
      '<div style="color:#6b6b80;font-size:11px;margin-top:3px;">' + pct + '% 已看完</div>' +
    '</div>';
  }
  container.innerHTML = html;

  // 点击进入剧集详情
  container.querySelectorAll('.video-card[data-series]').forEach(function(card) {
    card.onclick = function() {
      var seriesName = card.dataset.series;
      // 记住点击的剧集，返回时置顶
      state.lastClickedSeries = seriesName;
      state.currentView = 'series-episodes';
      state.currentSeries = seriesName;
      fetchVideos();
    };
  });
}

function renderVideoCard(videoWithTags) {
  var v = videoWithTags.video || videoWithTags;
  var tags = videoWithTags.tags || [];
  var title = v.title || v.file_name || '(unknown)';
  var favMark = v.is_favorite ? ' ★' : '';
  var ratingStr = (v.rating && v.rating > 0) ? v.rating.toFixed(1) : '-';
  var sizeStr = formatSize(v.file_size || 0);

  // 所有标签色块
  var tagsHtml = '';
  for (var j = 0; j < tags.length; j++) {
    var tc = tags[j].color || '#6366f1';
    tagsHtml += '<span class="tag-chip tag-chip-sm" style="background:' + tc + ';">' + tags[j].name + '</span>';
  }

  var line2 = sizeStr + '  ·  ★ ' + ratingStr;

  return '<div class="video-card" data-id="' + v.id + '">' +
    '<div class="video-card-title">' + title + favMark + '</div>' +
    '<div class="video-card-meta">' + line2 + '</div>' +
    (tagsHtml ? '<div class="video-card-tags">' + tagsHtml + '</div>' : '') +
  '</div>';
}

// ============================================
// 渲染历史记录
// ============================================
function renderHistory(history) {
  const container = document.getElementById('video-container');
  const emptyState = document.getElementById('empty-state');

  if (history.length === 0) {
    container.innerHTML = '';
    emptyState.classList.remove('hidden');
    return;
  }

  emptyState.classList.add('hidden');
  container.innerHTML = history.map(h => `
    <div class="video-card history-card" data-id="${h.video_id}">
      <div class="video-card-title">${h.video_title || '未知'}</div>
      <div class="video-card-meta">观看于 ${new Date(h.watched_at).toLocaleString('zh-CN')}</div>
    </div>
  `).join('');

  container.querySelectorAll('.video-card').forEach(card => {
    card.addEventListener('click', () => {
      const id = parseInt(card.dataset.id);
      // 将点击的卡片移到最前面
      container.insertBefore(card, container.firstChild);
      openVideoDetail(id);
    });
  });
}

// ============================================
// 渲染标签
// ============================================
function renderTags() {
  const tagList = document.getElementById('tag-list');
  tagList.innerHTML = state.tags.map(t => `
    <div class="tag-nav-item" data-tag-id="${t.id}">
      <span class="tag-drag-handle" title="拖拽排序">⠿</span>
      <span class="tag-dot" style="background:${t.color}"></span>
      <span>${t.name}</span>
      <span class="tag-count">${t.video_count || 0}</span>
    </div>
  `).join('');

  tagList.querySelectorAll('.tag-nav-item').forEach(item => {
    // 点击事件（默认多选，互斥标签例外）
    item.addEventListener('click', (e) => {
      // 如果点击的是拖拽手柄，不触发选中
      if (e.target.classList.contains('tag-drag-handle')) return;

      const tagId = parseInt(item.dataset.tagId);
      const tagName = item.querySelector('span:nth-child(3)').textContent.trim();

      // 不改变 currentView，标签作为次筛选器与主导航组合使用
      var idx = state.selectedTagIds.indexOf(tagId);
      if (idx >= 0) {
        // 已选中 → 取消选中
        state.selectedTagIds.splice(idx, 1);
      } else {
        // 未选中 → 加入选中
        // 互斥规则：已看过 ↔ 未看过 不能同时选中
        if (tagName === '已看过') {
          var unwatchedTag = state.tags.find(t => t.name === '未看过');
          if (unwatchedTag) {
            var ui = state.selectedTagIds.indexOf(unwatchedTag.id);
            if (ui >= 0) state.selectedTagIds.splice(ui, 1);
          }
        } else if (tagName === '未看过') {
          var watchedTag = state.tags.find(t => t.name === '已看过');
          if (watchedTag) {
            var wi = state.selectedTagIds.indexOf(watchedTag.id);
            if (wi >= 0) state.selectedTagIds.splice(wi, 1);
          }
        }
        state.selectedTagIds.push(tagId);
      }

      updateNavActive();
      highlightSelectedTags();
      fetchVideos();
    });

    // 拖拽手柄事件
    const handle = item.querySelector('.tag-drag-handle');
    handle.addEventListener('mousedown', (e) => {
      e.preventDefault();
      e.stopPropagation();
      startDrag(item, e);
    });
  });
}

// ============================================
// 拖拽排序
// ============================================
function startDrag(item, e) {
  if (!item || !e) return;
  var tagList = document.getElementById('tag-list');
  if (!tagList) return;
  var rect = item.getBoundingClientRect();

  dragState.dragging = true;
  dragState.dragItem = item;
  dragState.startY = e.clientY;
  dragState.itemTop = rect.top;
  dragState.itemHeight = rect.height;
  dragState.targetId = null;  // 记录目标位置的标签 ID

  // 设置元素为固定定位，跟随鼠标
  item.style.position = 'fixed';
  item.style.top = rect.top + 'px';
  item.style.left = rect.left + 'px';
  item.style.width = rect.width + 'px';
  item.style.zIndex = '1000';
  item.style.pointerEvents = 'none';
  item.classList.add('dragging');

  // 绑定全局事件
  document.addEventListener('mousemove', onDragMove);
  document.addEventListener('mouseup', onDragEnd);
}

function onDragMove(e) {
  if (!dragState.dragging || !dragState.dragItem) return;

  var dragItem = dragState.dragItem;
  var tagList = document.getElementById('tag-list');
  if (!tagList) return;

  // 更新拖拽元素位置
  var deltaY = e.clientY - dragState.startY;
  dragItem.style.top = (dragState.itemTop + deltaY) + 'px';

  // 找到最近的放置位置
  var items = Array.from(tagList.querySelectorAll('.tag-nav-item:not(.dragging)'));
  let closestItem = null;
  let closestDistance = Infinity;

  items.forEach(item => {
    const rect = item.getBoundingClientRect();
    const centerY = rect.top + rect.height / 2;
    const distance = Math.abs(e.clientY - centerY);

    if (distance < closestDistance) {
      closestDistance = distance;
      closestItem = item;
    }
  });

  if (closestItem) {
    const closestRect = closestItem.getBoundingClientRect();
    const closestCenter = closestRect.top + closestRect.height / 2;

    // 判断目标位置
    let targetId;
    if (e.clientY < closestCenter) {
      targetId = closestItem.dataset.tagId;
    } else {
      // 如果是后面，取下一个元素的 ID（如果没有下一个，用 'end'）
      const next = closestItem.nextElementSibling;
      if (next && next.classList.contains('tag-nav-item') && !next.classList.contains('dragging')) {
        targetId = next.dataset.tagId;
      } else {
        targetId = 'end';
      }
    }

    // 只有目标位置变化时才更新视觉提示
    if (targetId !== dragState.targetId) {
      // 移除之前的提示
      clearDragIndicators();

      // 添加新的提示
      if (targetId === 'end') {
        const lastItem = items[items.length - 1];
        if (lastItem) lastItem.classList.add('drag-after');
      } else {
        const targetEl = tagList.querySelector(`[data-tag-id="${targetId}"]`);
        if (targetEl) targetEl.classList.add('drag-before');
      }

      dragState.targetId = targetId;
    }
  }
}

async function onDragEnd(e) {
  if (!dragState.dragging) return;

  // 移除全局事件（先移除，防止重入）
  document.removeEventListener('mousemove', onDragMove);
  document.removeEventListener('mouseup', onDragEnd);

  try {
    const tagList = document.getElementById('tag-list');
    const dragItem = dragState.dragItem;
    const targetId = dragState.targetId;

    // 先清除所有拖拽高亮边框（插入前清除一次）
    clearDragIndicators();

    // 清除所有内联样式
    if (dragItem) {
      dragItem.removeAttribute('style');
      dragItem.classList.remove('dragging');
    }

    // 根据 targetId 将元素插入到正确位置
    if (tagList && dragItem && targetId) {
      if (targetId === 'end') {
        tagList.appendChild(dragItem);
      } else {
        const targetEl = tagList.querySelector('[data-tag-id="' + targetId + '"]');
        if (targetEl) {
          tagList.insertBefore(dragItem, targetEl);
        }
      }
    }

    // 插入完成后再清除一次，确保没有遗漏
    clearDragIndicators();

    // 恢复选中标签的高亮样式
    highlightSelectedTags();

    // 获取新的 DOM 顺序，更新 state.tags
    if (tagList) {
      var newOrder = Array.from(tagList.querySelectorAll('.tag-nav-item')).map(function(item) {
        return parseInt(item.dataset.tagId);
      });
      state.tags.sort(function(a, b) { return newOrder.indexOf(a.id) - newOrder.indexOf(b.id); });
    }

    // 保存新顺序到后端
    await saveTagOrder();
  } catch (err) {
    console.error('拖拽结束处理出错:', err);
  } finally {
    // 清理状态
    dragState.dragging = false;
    dragState.dragItem = null;
    dragState.targetId = null;
  }
}

function clearDragIndicators() {
  var tagList = document.getElementById('tag-list');
  if (!tagList) return;
  tagList.querySelectorAll('.drag-before').forEach(function(el) { el.classList.remove('drag-before'); });
  tagList.querySelectorAll('.drag-after').forEach(function(el) { el.classList.remove('drag-after'); });
}

async function saveTagOrder() {
  const orders = state.tags.map((tag, index) => [tag.id, index]);
  try {
    await invoke('update_tag_orders', { orders });
  } catch (e) {
    console.error('保存标签顺序失败:', e);
  }
}

function highlightSelectedTags() {
  document.querySelectorAll('.tag-nav-item').forEach(item => {
    var tagId = parseInt(item.dataset.tagId);
    if (state.selectedTagIds.indexOf(tagId) >= 0) {
      item.style.background = 'rgba(99, 102, 241, 0.2)';
      item.style.borderLeft = '3px solid #6366f1';
    } else {
      item.style.background = '';
      item.style.borderLeft = '';
    }
  });
}

// ============================================
// 更新导航栏
// ============================================
function updateNavActive() {
  document.querySelectorAll('.nav-item').forEach(item => {
    item.classList.remove('active');
    if (item.dataset.view === state.currentView) {
      item.classList.add('active');
    }
  });
}

// ============================================
// 更新统计信息
// ============================================
function updateStats() {
  const getVideo = (item) => item.video || item;
  const statTotal = document.getElementById('stat-total');
  const statSeries = document.getElementById('stat-series');
  const statSize = document.getElementById('stat-size');

  if (state.currentView === 'series-episodes') {
    // 剧集详情页：显示集数
    statTotal.textContent = `${state.totalCount} 集`;
    statSeries.classList.add('hidden');
    // 计算当前剧集的总大小
    let totalSize = 0;
    state.videos.forEach(v => {
      const video = v.video || v;
      totalSize += video.file_size || 0;
    });
    statSize.textContent = `大小 ${formatSize(totalSize)}`;
  } else if (state.currentView === 'series') {
    // 剧集概览页：显示剧集数量和总大小
    statTotal.textContent = `${state.totalCount} 个剧集`;
    statSeries.classList.add('hidden');
    // 计算所有剧集的总大小
    let totalSize = 0;
    state.seriesList.forEach(s => {
      totalSize += s.total_size || 0;
    });
    statSize.textContent = `总大小 ${formatSize(totalSize)}`;
  } else if (state.currentView === 'all') {
    // 电影页：显示电影数量和大小，隐藏剧集统计
    statTotal.textContent = `${state.totalCount} 部电影`;
    statSeries.classList.add('hidden');
    // 计算当前列表（电影）的总大小
    let totalSize = 0;
    state.videos.forEach(v => {
      const video = v.video || v;
      totalSize += video.file_size || 0;
    });
    statSize.textContent = `总大小 ${formatSize(totalSize)}`;
  } else {
    // 其他视图（收藏夹、历史等）：显示视频数量
    statTotal.textContent = `${state.totalCount} 个视频`;
    statSeries.classList.add('hidden');
    // 计算当前列表的总大小
    let totalSize = 0;
    state.videos.forEach(v => {
      const video = v.video || v;
      totalSize += video.file_size || 0;
    });
    statSize.textContent = `总大小 ${formatSize(totalSize)}`;
  }
}

function updateStatsDisplay(stats) {
  document.getElementById('count-all').textContent = stats.movie_count;
  document.getElementById('count-fav').textContent = stats.favorites_count;
  document.getElementById('count-series').textContent = stats.total_series;
  // 注意：stat-series 和 stat-size 现在由 updateStats() 根据当前视图动态更新
  // 这里不再设置它们，避免显示全局统计
}

// ============================================
// 视频详情对话框
// ============================================
function openVideoDetail(videoId) {
  const videoData = state.videos.find(v => (v.video || v).id === videoId);
  if (!videoData) return;

  state.selectedVideoId = videoId;
  const v = videoData.video || videoData;
  const tags = videoData.tags || [];

  document.getElementById('video-detail-title').textContent = v.title || v.file_name;
  document.getElementById('detail-title-input').value = v.title || v.file_name;
  document.getElementById('detail-path').textContent = v.file_path;
  document.getElementById('detail-size').textContent = formatSize(v.file_size);

  // 评分（事件委托，解决 innerHTML 替换后事件丢失问题）
  var ratingDiv = document.getElementById('detail-rating');
  ratingDiv.innerHTML = renderRatingInput(v.rating);
  ratingDiv.onclick = async function(e) {
    var btn = e.target.closest('.star-btn');
    if (!btn) return;
    var newRating = parseInt(btn.dataset.rating);
    try {
      await invoke('set_rating', { videoId: v.id, rating: newRating });
      v.rating = newRating;
      ratingDiv.innerHTML = renderRatingInput(newRating);
      showToast('评分已更新为 ' + newRating);
      fetchVideos();
    } catch (err) {
      showToast('评分失败: ' + err, 'error');
    }
  };

  // 标签
  const tagsDiv = document.getElementById('detail-tags');
  tagsDiv.innerHTML = tags.map(t =>
    `<span class="tag-chip" style="background:${t.color}">${t.name} <span data-tag-id="${t.id}" class="remove-tag" style="cursor:pointer;margin-left:4px">✕</span></span>`
  ).join('') + `<button class="btn btn-secondary" id="btn-add-tag-to-video" style="padding:4px 10px;font-size:12px">+ 添加标签</button>`;

  // 删除标签事件
  tagsDiv.querySelectorAll('.remove-tag').forEach(el => {
    el.addEventListener('click', async (e) => {
      e.stopPropagation();
      const tagId = parseInt(el.dataset.tagId);
      try {
        await invoke('remove_video_tag', { videoId: v.id, tagId });
        showToast('标签已移除');
        await fetchVideos();
        await fetchTags();
        openVideoDetail(videoId);
      } catch (e) {
        showToast('移除标签失败: ' + e, 'error');
      }
    });
  });

  // 添加标签按钮
  document.getElementById('btn-add-tag-to-video').addEventListener('click', () => {
    showTagSelector(v.id);
  });

  // 收藏按钮
  const favBtn = document.getElementById('btn-fav-toggle');
  favBtn.textContent = v.is_favorite ? '取消收藏' : '收藏';
  favBtn.className = v.is_favorite ? 'btn btn-secondary' : 'btn btn-warning';

  // 剧集模式下显示返回和全剧按钮
  var backBtn = document.getElementById('btn-back-series');
  var markBtn = document.getElementById('btn-mark-series-watched');
  if (state.currentView === 'series-episodes') {
    backBtn.classList.remove('hidden');
    markBtn.classList.remove('hidden');
  } else {
    backBtn.classList.add('hidden');
    markBtn.classList.add('hidden');
  }

  // 显示对话框
  document.getElementById('video-modal').classList.remove('hidden');
}

function showTagSelector(videoId) {
  // 已经绑定在该视频上的标签 ID
  var boundIds = [];
  document.querySelectorAll('#detail-tags .remove-tag').forEach(function(el) {
    boundIds.push(parseInt(el.dataset.tagId));
  });

  var available = state.tags.filter(function(t) { return boundIds.indexOf(t.id) === -1; });

  // 构建选择列表 HTML
  var listHtml = '';
  for (var i = 0; i < available.length; i++) {
    listHtml += '<div class="tag-select-item" data-tag-id="' + available[i].id + '" style="display:flex;align-items:center;gap:8px;padding:8px 12px;cursor:pointer;border-radius:6px;transition:background 0.15s;">' +
      '<span style="width:12px;height:12px;border-radius:50%;background:' + available[i].color + ';flex-shrink:0;"></span>' +
      '<span style="color:#e8e8e8;font-size:14px;">' + available[i].name + '</span>' +
    '</div>';
  }
  if (available.length === 0) {
    listHtml = '<div style="color:#6b6b80;font-size:13px;padding:8px;">暂无可用标签，请在下方创建</div>';
  }

  // 创建内联选择器
  var existing = document.getElementById('tag-selector-inline');
  if (existing) existing.remove();

  var selector = document.createElement('div');
  selector.id = 'tag-selector-inline';
  selector.style.cssText = 'background:#1a1a2e;border:1px solid #2d2d4a;border-radius:8px;padding:12px;margin-top:8px;';
  selector.innerHTML =
    '<div style="color:#a0a0b8;font-size:12px;margin-bottom:8px;">选择标签：</div>' +
    '<div id="tag-selector-list" style="max-height:200px;overflow-y:auto;">' + listHtml + '</div>' +
    '<div style="margin-top:10px;padding-top:10px;border-top:1px solid #2d2d4a;">' +
      '<div style="color:#a0a0b8;font-size:12px;margin-bottom:6px;">或创建新标签：</div>' +
      '<div style="display:flex;gap:6px;">' +
        '<input type="text" id="tag-selector-new-name" placeholder="标签名" style="flex:1;padding:6px 10px;background:#252540;border:1px solid #2d2d4a;border-radius:6px;color:#e8e8e8;font-size:13px;font-family:var(--font);" />' +
        '<input type="color" id="tag-selector-new-color" value="#6366f1" style="width:36px;height:32px;border:1px solid #2d2d4a;border-radius:6px;cursor:pointer;background:transparent;padding:2px;" />' +
        '<button id="tag-selector-create" style="padding:6px 12px;background:#6366f1;color:white;border:none;border-radius:6px;cursor:pointer;font-size:13px;">创建</button>' +
      '</div>' +
    '</div>';

  // 插入到详情标签区域后面
  var tagsDiv = document.getElementById('detail-tags');
  tagsDiv.parentNode.insertBefore(selector, tagsDiv.nextSibling);

  // 绑定已有标签点击
  selector.querySelectorAll('.tag-select-item').forEach(function(item) {
    item.onmouseenter = function() { item.style.background = '#252540'; };
    item.onmouseleave = function() { item.style.background = 'transparent'; };
    item.onclick = async function() {
      var tagId = parseInt(item.dataset.tagId);
      try {
        await invoke('add_video_tag', { videoId: videoId, tagId: tagId });
        showToast('标签已添加');
        selector.remove();
        await fetchVideos();
        await fetchTags();
        openVideoDetail(videoId);
      } catch (e) {
        showToast('添加失败: ' + e, 'error');
      }
    };
  });

  // 绑定创建新标签
  var createBtn = document.getElementById('tag-selector-create');
  if (createBtn) {
    createBtn.onclick = async function() {
      var nameInput = document.getElementById('tag-selector-new-name');
      var colorInput = document.getElementById('tag-selector-new-color');
      var name = nameInput.value.trim();
      if (!name) { showToast('请输入标签名', 'error'); return; }
      try {
        var newTag = await invoke('create_tag', { name: name, color: colorInput.value });
        await invoke('add_video_tag', { videoId: videoId, tagId: newTag.id });
        showToast('标签已创建并添加');
        selector.remove();
        await fetchVideos();
        await fetchTags();
        openVideoDetail(videoId);
      } catch (e) {
        showToast('创建失败: ' + e, 'error');
      }
    };
  }
}

function showSeriesTagSelector(seriesName) {
  var displayName = seriesName && seriesName.trim() !== '' ? seriesName : '未命名剧集';

  // 构建选择列表 HTML（剧集模式不需要排除已绑定的标签，因为是批量操作）
  var listHtml = '';
  for (var i = 0; i < state.tags.length; i++) {
    listHtml += '<div class="tag-select-item" data-tag-id="' + state.tags[i].id + '" style="display:flex;align-items:center;gap:8px;padding:8px 12px;cursor:pointer;border-radius:6px;transition:background 0.15s;">' +
      '<span style="width:12px;height:12px;border-radius:50%;background:' + state.tags[i].color + ';flex-shrink:0;"></span>' +
      '<span style="color:#e8e8e8;font-size:14px;">' + state.tags[i].name + '</span>' +
    '</div>';
  }
  if (state.tags.length === 0) {
    listHtml = '<div style="color:#6b6b80;font-size:13px;padding:8px;">暂无可用标签，请在下方创建</div>';
  }

  // 创建内联选择器
  var existing = document.getElementById('tag-selector-inline');
  if (existing) existing.remove();

  var selector = document.createElement('div');
  selector.id = 'tag-selector-inline';
  selector.style.cssText = 'position:fixed;top:50%;left:50%;transform:translate(-50%,-50%);z-index:10000;background:#1a1a2e;border:1px solid #2d2d4a;border-radius:12px;padding:20px;min-width:280px;box-shadow:0 8px 32px rgba(0,0,0,0.5);';
  selector.innerHTML =
    '<div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:12px;">' +
      '<div style="color:#e8e8e8;font-size:15px;font-weight:600;">给「' + displayName + '」打标签</div>' +
      '<div id="tag-selector-close" style="cursor:pointer;color:#6b6b80;font-size:18px;padding:4px;">✕</div>' +
    '</div>' +
    '<div style="color:#a0a0b8;font-size:12px;margin-bottom:8px;">选择标签（将应用到该剧集的所有视频）：</div>' +
    '<div id="tag-selector-list" style="max-height:250px;overflow-y:auto;margin-bottom:12px;">' + listHtml + '</div>' +
    '<div style="padding-top:12px;border-top:1px solid #2d2d4a;">' +
      '<div style="color:#a0a0b8;font-size:12px;margin-bottom:6px;">或创建新标签：</div>' +
      '<div style="display:flex;gap:6px;">' +
        '<input type="text" id="tag-selector-new-name" placeholder="标签名" style="flex:1;padding:6px 10px;background:#252540;border:1px solid #2d2d4a;border-radius:6px;color:#e8e8e8;font-size:13px;font-family:var(--font);" />' +
        '<input type="color" id="tag-selector-new-color" value="#6366f1" style="width:36px;height:32px;border:1px solid #2d2d4a;border-radius:6px;cursor:pointer;background:transparent;padding:2px;" />' +
        '<button id="tag-selector-create" style="padding:6px 12px;background:#6366f1;color:white;border:none;border-radius:6px;cursor:pointer;font-size:13px;">创建</button>' +
      '</div>' +
    '</div>';

  document.body.appendChild(selector);

  // 添加遮罩层
  var overlay = document.createElement('div');
  overlay.id = 'tag-selector-overlay';
  overlay.style.cssText = 'position:fixed;top:0;left:0;right:0;bottom:0;z-index:9999;background:rgba(0,0,0,0.5);';
  document.body.appendChild(overlay);

  // 关闭按钮
  var closeBtn = document.getElementById('tag-selector-close');
  if (closeBtn) {
    closeBtn.onclick = function() {
      selector.remove();
      overlay.remove();
    };
  }
  overlay.onclick = function() {
    selector.remove();
    overlay.remove();
  };

  // 绑定已有标签点击
  selector.querySelectorAll('.tag-select-item').forEach(function(item) {
    item.onmouseenter = function() { item.style.background = '#252540'; };
    item.onmouseleave = function() { item.style.background = 'transparent'; };
    item.onclick = async function() {
      var tagId = parseInt(item.dataset.tagId);
      try {
        var count = await invoke('add_series_tag', { seriesName: seriesName, tagId: tagId });
        showToast('已为 ' + count + ' 个视频添加标签');
        selector.remove();
        overlay.remove();
        await fetchVideos();
        await fetchTags();
      } catch (e) {
        showToast('添加失败: ' + e, 'error');
      }
    };
  });

  // 绑定创建新标签
  var createBtn = document.getElementById('tag-selector-create');
  if (createBtn) {
    createBtn.onclick = async function() {
      var nameInput = document.getElementById('tag-selector-new-name');
      var colorInput = document.getElementById('tag-selector-new-color');
      var name = nameInput.value.trim();
      if (!name) { showToast('请输入标签名', 'error'); return; }
      try {
        var newTag = await invoke('create_tag', { name: name, color: colorInput.value });
        var count = await invoke('add_series_tag', { seriesName: seriesName, tagId: newTag.id });
        showToast('已创建标签并为 ' + count + ' 个视频添加');
        selector.remove();
        overlay.remove();
        await fetchVideos();
        await fetchTags();
      } catch (e) {
        showToast('创建失败: ' + e, 'error');
      }
    };
  }
}

function showSeriesRatingSelector(seriesName) {
  var displayName = seriesName && seriesName.trim() !== '' ? seriesName : '未命名剧集';

  // 查找该剧集当前的评分（从剧集概览中获取）
  var currentRating = 0;
  var seriesData = state.seriesList && state.seriesList.find(function(s) { return s.name === seriesName; });
  if (seriesData) {
    currentRating = seriesData.rating || 0;
  }

  // 创建评分选择器
  var existing = document.getElementById('rating-selector-inline');
  if (existing) existing.remove();

  var selector = document.createElement('div');
  selector.id = 'rating-selector-inline';
  selector.style.cssText = 'position:fixed;top:50%;left:50%;transform:translate(-50%,-50%);z-index:10000;background:#1a1a2e;border:1px solid #2d2d4a;border-radius:12px;padding:20px;min-width:300px;box-shadow:0 8px 32px rgba(0,0,0,0.5);';

  var starsHtml = '<div style="display:flex;gap:4px;margin:12px 0;">';
  for (var i = 1; i <= 10; i++) {
    var activeClass = i <= currentRating ? 'background:#fbbf24;' : 'background:#252540;';
    starsHtml += '<button class="series-rating-star" data-rating="' + i + '" style="width:28px;height:28px;border:1px solid #2d2d4a;border-radius:4px;cursor:pointer;font-size:16px;color:' + (i <= currentRating ? '#fbbf24' : '#6b6b80') + ';' + activeClass + '">' + (i <= currentRating ? '★' : '☆') + '</button>';
  }
  starsHtml += '</div>';

  selector.innerHTML =
    '<div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:8px;">' +
      '<div style="color:#e8e8e8;font-size:15px;font-weight:600;">给「' + displayName + '」评分</div>' +
      '<div id="rating-selector-close" style="cursor:pointer;color:#6b6b80;font-size:18px;padding:4px;">✕</div>' +
    '</div>' +
    '<div style="color:#a0a0b8;font-size:12px;margin-bottom:4px;">评分将应用到该剧集的所有视频（1-10 分）</div>' +
    starsHtml +
    '<div style="color:#a0a0b8;font-size:13px;text-align:center;">当前评分：<span id="rating-display" style="color:#fbbf24;font-weight:600;">' + (currentRating > 0 ? currentRating + ' 分' : '未评分') + '</span></div>' +
    '<div style="margin-top:12px;text-align:center;">' +
      '<button id="rating-selector-clear" style="padding:6px 12px;background:#252540;color:#a0a0b8;border:1px solid #2d2d4a;border-radius:6px;cursor:pointer;font-size:13px;margin-right:8px;">清除评分</button>' +
      '<button id="rating-selector-confirm" style="padding:6px 16px;background:#6366f1;color:white;border:none;border-radius:6px;cursor:pointer;font-size:13px;">确定</button>' +
    '</div>';

  document.body.appendChild(selector);

  // 添加遮罩层
  var overlay = document.createElement('div');
  overlay.id = 'rating-selector-overlay';
  overlay.style.cssText = 'position:fixed;top:0;left:0;right:0;bottom:0;z-index:9999;background:rgba(0,0,0,0.5);';
  document.body.appendChild(overlay);

  var selectedRating = currentRating;

  // 关闭按钮
  var closeBtn = document.getElementById('rating-selector-close');
  if (closeBtn) {
    closeBtn.onclick = function() {
      selector.remove();
      overlay.remove();
    };
  }
  overlay.onclick = function() {
    selector.remove();
    overlay.remove();
  };

  // 星星点击
  selector.querySelectorAll('.series-rating-star').forEach(function(btn) {
    btn.onclick = function() {
      selectedRating = parseInt(btn.dataset.rating);
      // 更新显示
      selector.querySelectorAll('.series-rating-star').forEach(function(b) {
        var r = parseInt(b.dataset.rating);
        if (r <= selectedRating) {
          b.style.background = '#fbbf24';
          b.style.color = '#fbbf24';
          b.textContent = '★';
        } else {
          b.style.background = '#252540';
          b.style.color = '#6b6b80';
          b.textContent = '☆';
        }
      });
      document.getElementById('rating-display').textContent = selectedRating + ' 分';
    };
  });

  // 清除评分
  var clearBtn = document.getElementById('rating-selector-clear');
  if (clearBtn) {
    clearBtn.onclick = async function() {
      try {
        await invoke('set_series_rating', { seriesName: seriesName, rating: 0 });
        showToast('已清除「' + displayName + '」的评分');
        selector.remove();
        overlay.remove();
        fetchVideos();
      } catch (e) {
        showToast('操作失败: ' + e, 'error');
      }
    };
  }

  // 确定按钮
  var confirmBtn = document.getElementById('rating-selector-confirm');
  if (confirmBtn) {
    confirmBtn.onclick = async function() {
      if (selectedRating <= 0) {
        showToast('请选择评分', 'error');
        return;
      }
      try {
        var count = await invoke('set_series_rating', { seriesName: seriesName, rating: selectedRating });
        showToast('已为「' + displayName + '」的 ' + count + ' 个视频评 ' + selectedRating + ' 分');
        selector.remove();
        overlay.remove();
        fetchVideos();
      } catch (e) {
        showToast('评分失败: ' + e, 'error');
      }
    };
  }
}

// ============================================
// 扫描对话框
// ============================================
function openScanDialog() {
  document.getElementById('scan-modal').classList.remove('hidden');
  document.getElementById('scan-result').classList.add('hidden');
  document.getElementById('scan-progress').classList.add('hidden');
  document.getElementById('scan-path').value = '';
  loadScanFolders();
}

async function loadScanFolders() {
  const list = document.getElementById('scan-folders-list');
  const section = document.getElementById('scan-folders-section');
  try {
    const folders = await invoke('get_scan_folders');
    if (!folders || folders.length === 0) {
      section.style.display = 'none';
      return;
    }
    section.style.display = '';
    list.innerHTML = folders.map(f => `
      <div class="scan-folder-item" data-path="${escapeHtml(f.folder_path)}">
        <span class="scan-folder-path" title="${escapeHtml(f.folder_path)}">${escapeHtml(f.folder_path)}</span>
        <span class="scan-folder-count">${f.video_count} 个视频</span>
        <button class="scan-folder-delete" data-id="${f.id}" title="移除记录">✕</button>
      </div>
    `).join('');

    // 点击路径：填入扫描输入框
    list.querySelectorAll('.scan-folder-path').forEach(el => {
      el.addEventListener('click', () => {
        document.getElementById('scan-path').value = el.closest('.scan-folder-item').dataset.path;
      });
    });

    // 删除记录
    list.querySelectorAll('.scan-folder-delete').forEach(btn => {
      btn.addEventListener('click', async (e) => {
        e.stopPropagation();
        const id = parseInt(btn.dataset.id);
        try {
          await invoke('delete_scan_folder', { folderId: id });
          loadScanFolders();
        } catch (e) {
          showToast('删除失败: ' + e, 'error');
        }
      });
    });
  } catch (e) {
    section.style.display = 'none';
  }
}

async function startScan() {
  const path = document.getElementById('scan-path').value;
  if (!path) {
    showToast('请先选择文件夹', 'error');
    return;
  }

  const incremental = document.getElementById('incremental-scan').checked;
  const defaultWatched = document.getElementById('scan-default-watched').checked;

  document.getElementById('scan-progress').classList.remove('hidden');
  document.getElementById('scan-result').classList.add('hidden');
  document.getElementById('btn-start-scan').disabled = true;
  document.getElementById('scan-status').textContent = '扫描中...';
  document.getElementById('progress-fill').style.width = '50%';

  try {
    var asSeries = document.getElementById('scan-as-series').checked;
    var result;

    if (asSeries) {
      // 扫描为剧集：用文件夹名作为剧集名
      var folderName = path.split('\\').pop().split('/').pop();
      result = await invoke('scan_series', { dirPath: path, seriesName: folderName, incremental, defaultWatched });
    } else {
      result = await invoke('scan_videos', { dirPath: path, incremental, defaultWatched });
    }

    document.getElementById('progress-fill').style.width = '100%';
    document.getElementById('scan-status').textContent = '扫描完成！';

    const resultDiv = document.getElementById('scan-result');
    resultDiv.classList.remove('hidden');
    resultDiv.innerHTML = `
      <p>✅ 扫描完成</p>
      <p>找到 ${result.total_found} 个视频文件</p>
      <p>新增 ${result.new_added} 个</p>
      ${result.series_detected.length > 0 ? `<p>检测到 ${result.series_detected.length} 个剧集</p>` : ''}
      ${result.errors.length > 0 ? `<p style="color:var(--danger)">❌ ${result.errors.length} 个错误</p>` : ''}
    `;

    fetchVideos();
    fetchTags();
    fetchStats();
    loadScanFolders();
    showToast(`扫描完成，新增 ${result.new_added} 个视频`);
  } catch (e) {
    document.getElementById('scan-status').textContent = '扫描失败';
    showToast('扫描失败: ' + e, 'error');
  } finally {
    document.getElementById('btn-start-scan').disabled = false;
  }
}

// 刷新所有扫描位置
async function refreshAllScans() {
  const btn = document.getElementById('btn-refresh');
  if (btn.disabled) return;
  btn.disabled = true;
  const span = btn.querySelector('span');
  const originalText = span.textContent;
  span.textContent = '刷新中...';

  try {
    const result = await invoke('refresh_all_scans', { defaultWatched: false });
    const parts = [];
    if (result.removed > 0) parts.push(`移除 ${result.removed} 个失效记录`);
    if (result.new_added > 0) parts.push(`新增 ${result.new_added} 个视频`);
    if (result.folders_scanned > 0) parts.push(`扫描 ${result.folders_scanned} 个文件夹`);

    if (parts.length > 0) {
      showToast('刷新完成：' + parts.join('，'));
    } else {
      showToast('刷新完成，无变化');
    }

    if (result.errors.length > 0) {
      console.warn('刷新错误:', result.errors);
    }

    fetchVideos();
    fetchTags();
    fetchStats();
  } catch (e) {
    showToast('刷新失败: ' + e, 'error');
  } finally {
    btn.disabled = false;
    span.textContent = originalText;
  }
}

// ============================================
// 标签管理对话框
// ============================================
function openTagModal() {
  document.getElementById('tag-modal').classList.remove('hidden');
  renderTagManageList();
}

function renderTagManageList() {
  const list = document.getElementById('tag-manage-list');
  list.innerHTML = state.tags.map(t => `
    <div class="tag-manage-item">
      <span class="tag-dot" style="background:${t.color}"></span>
      <span class="tag-name">${t.name}</span>
      <span class="tag-video-count">${t.video_count || 0} 个视频</span>
      <button class="btn-delete-tag" data-tag-id="${t.id}">🗑</button>
    </div>
  `).join('');

  list.querySelectorAll('.btn-delete-tag').forEach(btn => {
    btn.addEventListener('click', async () => {
      const tagId = parseInt(btn.dataset.tagId);
      var ok = await showConfirm('删除标签', '确定删除此标签？');
      if (ok) {
        try {
          await invoke('delete_tag', { tagId });
          showToast('标签已删除');
          // 从已选标签中移除被删除的标签
          state.selectedTagIds = state.selectedTagIds.filter(id => id !== tagId);
          fetchTags();
          fetchVideos();
          renderTagManageList();
        } catch (e) {
          showToast('删除标签失败: ' + e, 'error');
        }
      }
    });
  });
}

async function createNewTag() {
  const name = document.getElementById('new-tag-name').value.trim();
  const color = document.getElementById('new-tag-color').value;

  if (!name) {
    showToast('请输入标签名称', 'error');
    return;
  }

  try {
    await invoke('create_tag', { name, color });
    document.getElementById('new-tag-name').value = '';
    showToast('标签已创建');
    fetchTags();
    renderTagManageList();
  } catch (e) {
    showToast('创建标签失败: ' + e, 'error');
  }
}

// ============================================
// 导出数据
// ============================================
async function exportData() {
  try {
    const data = await invoke('export_json');
    const json = JSON.stringify(data, null, 2);
    const blob = new Blob([json], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    const filename = `movieui-export-${new Date().toISOString().slice(0, 10)}.json`;
    a.href = url;
    a.download = filename;
    a.click();
    URL.revokeObjectURL(url);

    // 尝试获取下载路径
    const downloadDir = window.electron?.app?.getPath?.('downloads') || '';
    showToast(`✅ 导出完成！文件名: ${filename}（通常在浏览器下载文件夹中）`);
  } catch (e) {
    showToast('导出失败: ' + e, 'error');
  }
}

// ============================================
// 事件绑定
// ============================================
function bindEvents() {
  // 导航
  document.querySelectorAll('.nav-item').forEach(item => {
    item.addEventListener('click', (e) => {
      e.preventDefault();
      state.previousView = item.dataset.view;
      state.currentView = item.dataset.view;
      state.selectedTagIds = [];
      state.searchQuery = '';
      document.getElementById('search-input').value = '';
      updateNavActive();
      highlightSelectedTags();

      if (state.currentView === 'history') {
        fetchHistory();
      } else {
        fetchVideos();
      }
    });
  });

  // 搜索
  let searchTimeout;
  document.getElementById('search-input').addEventListener('input', (e) => {
    clearTimeout(searchTimeout);
    state.searchQuery = e.target.value;
    searchTimeout = setTimeout(() => {
      // 在剧集详情页搜索时，回到剧集概览进行搜索
      if (state.currentView === 'series-episodes') {
        state.currentView = 'series';
        updateNavActive();
      }
      fetchVideos();
    }, 300);
  });

  // 排序
  document.getElementById('sort-select').addEventListener('change', (e) => {
    state.sortBy = e.target.value;
    fetchVideos();
  });

  // 评分过滤
  document.getElementById('rating-filter').addEventListener('change', (e) => {
    state.ratingFilter = parseInt(e.target.value);
    fetchVideos();
  });

  // 视图切换
  document.getElementById('btn-grid').addEventListener('click', () => {
    state.viewMode = 'grid';
    document.getElementById('video-container').classList.remove('list-view');
    document.getElementById('btn-grid').classList.add('active');
    document.getElementById('btn-list').classList.remove('active');
  });

  document.getElementById('btn-list').addEventListener('click', () => {
    state.viewMode = 'list';
    document.getElementById('video-container').classList.add('list-view');
    document.getElementById('btn-list').classList.add('active');
    document.getElementById('btn-grid').classList.remove('active');
  });

  // 扫描
  document.getElementById('btn-scan').addEventListener('click', openScanDialog);

  // 刷新
  document.getElementById('btn-refresh').addEventListener('click', refreshAllScans);
  document.getElementById('btn-browse-scan').onclick = async function() {
    try {
      var selected = await open({ directory: true, multiple: false });
      if (selected) {
        document.getElementById('scan-path').value = selected;
      }
    } catch (e) {
      console.error('选择文件夹失败:', e);
    }
  };
  document.getElementById('btn-start-scan').addEventListener('click', startScan);
  document.getElementById('btn-cancel-scan').addEventListener('click', () => {
    document.getElementById('scan-modal').classList.add('hidden');
  });
  document.getElementById('scan-close').addEventListener('click', () => {
    document.getElementById('scan-modal').classList.add('hidden');
  });

  // 标签管理
  document.getElementById('btn-add-tag').addEventListener('click', createNewTag);
  document.getElementById('tag-close').addEventListener('click', () => {
    document.getElementById('tag-modal').classList.add('hidden');
  });

  // 视频详情
  document.getElementById('video-close').addEventListener('click', () => {
    document.getElementById('video-modal').classList.add('hidden');
  });

  document.getElementById('btn-play').addEventListener('click', async () => {
    const item = state.videos.find(v => (v.video || v).id === state.selectedVideoId);
    if (item) {
      const v = item.video || item;
      try {
        await invoke('open_file', { path: v.file_path });
        await invoke('update_progress', { videoId: v.id, progress: 0 });
      } catch (e) {
        showToast('播放失败: ' + e, 'error');
      }
    }
  });

  document.getElementById('btn-open-folder').addEventListener('click', async () => {
    const item = state.videos.find(v => (v.video || v).id === state.selectedVideoId);
    if (item) {
      const v = item.video || item;
      try {
        await invoke('open_folder', { path: v.file_path });
      } catch (e) {
        showToast('打开文件夹失败: ' + e, 'error');
      }
    }
  });

  document.getElementById('btn-fav-toggle').addEventListener('click', async () => {
    try {
      const result = await invoke('toggle_favorite', { videoId: state.selectedVideoId });
      showToast(result ? '已收藏' : '已取消收藏');
      fetchVideos();
      fetchStats();
      document.getElementById('video-modal').classList.add('hidden');
    } catch (e) {
      showToast('操作失败: ' + e, 'error');
    }
  });

  document.getElementById('btn-delete-video').addEventListener('click', async () => {
    var ok = await showConfirm('删除记录', '确定删除此视频记录？\n\n不会删除本地文件。');
    if (ok) {
      try {
        await invoke('delete_video', { videoId: state.selectedVideoId });
        showToast('已删除记录');
        document.getElementById('video-modal').classList.add('hidden');
        fetchVideos();
        fetchTags();
        fetchStats();
      } catch (e) {
        showToast('删除失败: ' + e, 'error');
      }
    }
  });

  document.getElementById('btn-delete-file').addEventListener('click', async () => {
    var item = state.videos.find(v => (v.video || v).id === state.selectedVideoId);
    var filePath = item ? (item.video || item).file_path : '';
    var ok = await showConfirm('⚠️ 删除文件', '确定删除此视频文件？\n\n文件: ' + filePath + '\n\n此操作不可撤销！');
    if (ok) {
      try {
        var msg = await invoke('delete_video_with_file', { videoId: state.selectedVideoId });
        showToast(msg);
        document.getElementById('video-modal').classList.add('hidden');
        fetchVideos();
        fetchTags();
        fetchStats();
      } catch (e) {
        showToast('删除失败: ' + e, 'error');
      }
    }
  });

  // 导出
  document.getElementById('btn-export').addEventListener('click', exportData);

  // 导入
  document.getElementById('btn-import').addEventListener('click', async function() {
    try {
      var filePath = await open({
        multiple: false,
        filters: [{ name: 'JSON', extensions: ['json'] }]
      });
      if (!filePath) return;

      // 通过 Rust 命令读取文件内容
      var jsonStr = await invoke('read_text_file', { filePath: filePath });

      var result = await invoke('import_json', { jsonStr: jsonStr });
      showToast('导入完成: 新增 ' + result.imported + ' 条, 跳过 ' + result.skipped + ' 条');
      fetchVideos();
      fetchTags();
      fetchStats();
    } catch (e) {
      showToast('导入失败: ' + e, 'error');
    }
  });

  // 返回剧集列表（详情对话框中的按钮）
  document.getElementById('btn-back-series').onclick = function() {
    state.currentView = 'series';
    document.getElementById('video-modal').classList.add('hidden');
    fetchVideos();
  };

  // 面包屑导航：返回剧集概览
  function goBackToSeriesOverview() {
    state.currentView = 'series';
    fetchVideos();
  }
  document.getElementById('btn-back-series-overview').onclick = goBackToSeriesOverview;
  document.getElementById('breadcrumb-parent').onclick = goBackToSeriesOverview;

  // 全剧已看过
  document.getElementById('btn-mark-series-watched').onclick = async function() {
    var ok = await showConfirm('标记全剧', '将该剧集所有集标记为已看过？');
    if (ok) {
      try {
        var count = await invoke('mark_series_watched', { series: state.currentSeries, watched: true });
        showToast('已标记 ' + count + ' 集为已看过');
        fetchVideos();
        fetchTags();
        document.getElementById('video-modal').classList.add('hidden');
      } catch (e) {
        showToast('操作失败: ' + e, 'error');
      }
    }
  };

  // 切换已看过/未看过
  document.getElementById('btn-watched-toggle').onclick = async function() {
    try {
      var newStatus = await invoke('toggle_watched', { videoId: state.selectedVideoId });
      showToast('已标记为: ' + newStatus);
      fetchVideos();
      fetchTags();
      document.getElementById('video-modal').classList.add('hidden');
    } catch (e) {
      showToast('操作失败: ' + e, 'error');
    }
  };

  // 改名
  document.getElementById('btn-rename').onclick = async function() {
    var input = document.getElementById('detail-title-input');
    var newTitle = input.value.trim();
    if (!newTitle) { showToast('标题不能为空', 'error'); return; }
    try {
      await invoke('rename_video', { videoId: state.selectedVideoId, newTitle: newTitle });
      showToast('已改名');
      fetchVideos();
      document.getElementById('video-modal').classList.add('hidden');
    } catch (e) {
      showToast('改名失败: ' + e, 'error');
    }
  };

  // 关闭对话框（点击遮罩）
  document.querySelectorAll('.modal-overlay').forEach(overlay => {
    overlay.addEventListener('click', () => {
      overlay.closest('.modal').classList.add('hidden');
    });
  });
}

// ============================================
// 初始化
// ============================================
// ============================================
// 自定义右键菜单
// ============================================
var contextMenuTarget = null;
var contextMenuIsSeries = false;  // 是否是剧集卡片
var contextMenuSeriesName = '';   // 剧集名称

function initContextMenu() {
  var menu = document.getElementById('context-menu');
  var tagMenu = document.getElementById('tag-context-menu');
  if (!menu) return;

  // 全局拦截右键
  window.addEventListener('contextmenu', function(e) {
    e.preventDefault();
    // 先隐藏所有菜单
    menu.style.display = 'none';
    if (tagMenu) tagMenu.style.display = 'none';

    var card = e.target.closest('.video-card');
    var tagItem = e.target.closest('.tag-nav-item');

    if (card) {
      // 判断是剧集卡片还是视频卡片（使用 hasAttribute 判断，因为空字符串也是有效值）
      if (card.hasAttribute('data-series')) {
        // 剧集概览卡片
        contextMenuIsSeries = true;
        contextMenuSeriesName = card.dataset.series || '';
        contextMenuTarget = null;
      } else {
        // 普通视频卡片
        contextMenuIsSeries = false;
        contextMenuSeriesName = '';
        contextMenuTarget = parseInt(card.dataset.id);
      }

      var vitem = state.videos.find(function(v) { return (v.video || v).id === contextMenuTarget; });
      var v = vitem ? (vitem.video || vitem) : null;
      var favItem = menu.querySelector('[data-action="fav"]');
      if (favItem && v) {
        favItem.textContent = v.is_favorite ? '★ 取消收藏' : '☆ 收藏';
      }

      // 剧集卡片时隐藏某些菜单项
      menu.querySelectorAll('.ctx-item').forEach(function(item) {
        var action = item.dataset.action;
        if (contextMenuIsSeries) {
          // 剧集卡片显示：改名、添加标签、已看过、评分、删除记录、删除文件
          if (action === 'rename' || action === 'tag' || action === 'watched' || action === 'rating' || action === 'delete' || action === 'delete-file') {
            item.style.display = '';
          } else {
            item.style.display = 'none';
          }
        } else {
          item.style.display = '';
        }
      });

      menu.style.display = 'block';
      menu.style.left = Math.min(e.clientX, window.innerWidth - 180) + 'px';
      menu.style.top = Math.min(e.clientY, window.innerHeight - 250) + 'px';
    } else if (tagMenu && (e.target.closest('#tag-list') || e.target.closest('.nav-section-title'))) {
      // 标签区域右键（包括空白处）
      tagContextTarget = tagItem ? parseInt(tagItem.dataset.tagId) : null;

      // 显示/隐藏需要目标的菜单项
      tagMenu.querySelectorAll('.tag-ctx-need-target').forEach(function(el) {
        el.style.display = tagContextTarget ? '' : 'none';
      });

      tagMenu.style.display = 'block';
      tagMenu.style.left = Math.min(e.clientX, window.innerWidth - 180) + 'px';
      tagMenu.style.top = Math.min(e.clientY, window.innerHeight - 200) + 'px';
    }
  }, true);

  // 点击其他地方关闭所有菜单
  document.addEventListener('click', function(e) {
    if (!menu.contains(e.target)) menu.style.display = 'none';
    if (tagMenu && !tagMenu.contains(e.target)) tagMenu.style.display = 'none';
  });

  // 菜单项点击
  menu.querySelectorAll('.ctx-item').forEach(function(item) {
    item.onmouseenter = function() { item.style.background = '#252540'; };
    item.onmouseleave = function() { item.style.background = 'transparent'; };
    item.onclick = async function() {
      var action = item.dataset.action;
      menu.style.display = 'none';

      // 剧集卡片的处理
      if (contextMenuIsSeries) {
        if (action === 'rename') {
          var displayName = contextMenuSeriesName && contextMenuSeriesName.trim() !== '' ? contextMenuSeriesName : '未命名剧集';
          var newName = prompt('新剧集名:', displayName);
          if (newName && newName.trim() && newName.trim() !== displayName) {
            try {
              await invoke('rename_series', { oldName: contextMenuSeriesName, newName: newName.trim() });
              showToast('剧集已改名');
              fetchVideos();
              fetchStats();
            } catch (e) {
              showToast('改名失败: ' + e, 'error');
            }
          }
        } else if (action === 'tag') {
          // 给剧集打标签
          showSeriesTagSelector(contextMenuSeriesName);
          return;
        } else if (action === 'delete') {
          var displayName2 = contextMenuSeriesName && contextMenuSeriesName.trim() !== '' ? contextMenuSeriesName : '未命名剧集';
          var delOk = await showConfirm('删除剧集记录', '确定删除「' + displayName2 + '」的所有记录？\n\n不会删除本地文件。');
          if (delOk) {
            try {
              await invoke('delete_series', { seriesName: contextMenuSeriesName });
              showToast('剧集记录已删除');
              fetchVideos();
              fetchTags();
              fetchStats();
            } catch (e) {
              showToast('删除失败: ' + e, 'error');
            }
          }
        } else if (action === 'delete-file') {
          // 删除剧集记录和文件
          var displayName3 = contextMenuSeriesName && contextMenuSeriesName.trim() !== '' ? contextMenuSeriesName : '未命名剧集';
          var delFileOk = await showConfirm('删除剧集文件', '确定删除「' + displayName3 + '」的所有记录和本地文件？\n\n⚠️ 此操作不可恢复！');
          if (delFileOk) {
            try {
              var result = await invoke('delete_series_with_files', { seriesName: contextMenuSeriesName });
              showToast(result);
              fetchVideos();
              fetchTags();
              fetchStats();
            } catch (e) {
              showToast('删除失败: ' + e, 'error');
            }
          }
        } else if (action === 'watched') {
          // 切换剧集已看过状态
          try {
            var displayName4 = contextMenuSeriesName && contextMenuSeriesName.trim() !== '' ? contextMenuSeriesName : '未命名剧集';
            var count = await invoke('mark_series_watched', { series: contextMenuSeriesName, watched: true });
            showToast('已标记「' + displayName4 + '」为已看过 (' + count + ' 集)');
            fetchVideos();
            fetchTags();
            fetchStats();
          } catch (e) {
            showToast('操作失败: ' + e, 'error');
          }
        } else if (action === 'rating') {
          // 给剧集评分
          showSeriesRatingSelector(contextMenuSeriesName);
        }
        return;
      }

      // 普通视频卡片的处理
      if (!contextMenuTarget) return;

      switch (action) {
        case 'play':
          var v = state.videos.find(function(x) { return (x.video || x).id === contextMenuTarget; });
          if (v) {
            var vid = v.video || v;
            await invoke('open_file', { path: vid.file_path });
            await invoke('update_progress', { videoId: vid.id, progress: 0 });
          }
          break;
        case 'rename':
          openVideoDetail(contextMenuTarget);
          setTimeout(function() {
            var input = document.getElementById('detail-title-input');
            if (input) input.focus();
          }, 100);
          break;
        case 'tag':
          openVideoDetail(contextMenuTarget);
          setTimeout(function() { showTagSelector(contextMenuTarget); }, 100);
          break;
        case 'fav':
          await invoke('toggle_favorite', { videoId: contextMenuTarget });
          showToast('已切换收藏');
          fetchVideos();
          fetchStats();
          break;
        case 'watched':
          var ws = await invoke('toggle_watched', { videoId: contextMenuTarget });
          showToast('已标记: ' + ws);
          fetchVideos();
          fetchTags();
          break;
        case 'folder':
          var v2 = state.videos.find(function(x) { return (x.video || x).id === contextMenuTarget; });
          if (v2) await invoke('open_folder', { path: (v2.video || v2).file_path });
          break;
        case 'delete':
          var delOk2 = await showConfirm('删除记录', '确定删除此记录？');
          if (delOk2) {
            await invoke('delete_video', { videoId: contextMenuTarget });
            showToast('已删除');
            fetchVideos();
            fetchTags();
            fetchStats();
          }
          break;
      }
    };
  });

  // 标签菜单项点击
  if (tagMenu) {
    tagMenu.querySelectorAll('.tag-ctx-item').forEach(function(item) {
      item.onmouseenter = function() { item.style.background = '#252540'; };
      item.onmouseleave = function() { item.style.background = 'transparent'; };
      item.onclick = async function() {
        var action = item.dataset.action;
        tagMenu.style.display = 'none';
        if (!tagContextTarget) return;

        switch (action) {
          case 'new-tag':
            var newName = prompt('标签名:');
            if (newName && newName.trim()) {
              // 弹出带确认按钮的颜色选择弹窗
              var overlay2 = document.createElement('div');
              overlay2.style.cssText = 'position:fixed;top:0;left:0;width:100%;height:100%;background:rgba(0,0,0,0.5);z-index:10000;display:flex;align-items:center;justify-content:center;';
              var popup2 = document.createElement('div');
              popup2.style.cssText = 'background:#1e1e3a;border:1px solid #2d2d4a;border-radius:12px;padding:20px;min-width:240px;text-align:center;';
              popup2.innerHTML =
                '<div style="color:#a0a0b8;font-size:13px;margin-bottom:12px;">选择标签颜色</div>' +
                '<input type="color" id="new-tag-color-picker" value="#6366f1" style="width:100%;height:120px;border:1px solid #2d2d4a;border-radius:8px;cursor:pointer;background:transparent;padding:2px;" />' +
                '<div id="new-tag-color-hex" style="color:#e8e8e8;font-size:13px;margin-top:8px;font-family:monospace;">#6366f1</div>' +
                '<div style="display:flex;gap:8px;margin-top:14px;justify-content:center;">' +
                  '<button id="new-tag-color-cancel" style="padding:8px 20px;background:#252540;color:#a0a0b8;border:1px solid #2d2d4a;border-radius:6px;cursor:pointer;font-size:13px;">取消</button>' +
                  '<button id="new-tag-color-confirm" style="padding:8px 20px;background:#6366f1;color:white;border:none;border-radius:6px;cursor:pointer;font-size:13px;">确认</button>' +
                '</div>';
              overlay2.appendChild(popup2);
              document.body.appendChild(overlay2);

              var pickerInput2 = popup2.querySelector('#new-tag-color-picker');
              var hexLabel2 = popup2.querySelector('#new-tag-color-hex');
              pickerInput2.addEventListener('input', function() {
                hexLabel2.textContent = pickerInput2.value;
              });
              popup2.querySelector('#new-tag-color-cancel').addEventListener('click', function() {
                document.body.removeChild(overlay2);
              });
              overlay2.addEventListener('click', function(e) {
                if (e.target === overlay2) document.body.removeChild(overlay2);
              });
              popup2.querySelector('#new-tag-color-confirm').addEventListener('click', async function() {
                var pickedColor = pickerInput2.value;
                document.body.removeChild(overlay2);
                try {
                  await invoke('create_tag', { name: newName.trim(), color: pickedColor });
                  showToast('已创建');
                  fetchTags();
                } catch (e) { showToast('失败: ' + e, 'error'); }
              });
            }
            break;
          case 'filter':
            // 添加标签到选中列表（作为次筛选器）
            if (tagContextTarget) {
              var idx = state.selectedTagIds.indexOf(tagContextTarget);
              if (idx < 0) {
                state.selectedTagIds.push(tagContextTarget);
              }
            }
            updateNavActive();
            highlightSelectedTags();
            fetchVideos();
            break;
          case 'rename-tag':
            var tag = state.tags.find(function(t) { return t.id === tagContextTarget; });
            if (tag) {
              var newName = prompt('新标签名:', tag.name);
              if (newName && newName.trim() && newName.trim() !== tag.name) {
                try {
                  await invoke('update_tag', { tagId: tagContextTarget, name: newName.trim(), color: null });
                  showToast('已重命名');
                  fetchTags();
                } catch (e) { showToast('失败: ' + e, 'error'); }
              }
            }
            break;
          case 'change-color':
            var tag2 = state.tags.find(function(t) { return t.id === tagContextTarget; });
            if (tag2) {
              // 创建带确认按钮的颜色选择弹窗
              var overlay = document.createElement('div');
              overlay.style.cssText = 'position:fixed;top:0;left:0;width:100%;height:100%;background:rgba(0,0,0,0.5);z-index:10000;display:flex;align-items:center;justify-content:center;';
              var popup = document.createElement('div');
              popup.style.cssText = 'background:#1e1e3a;border:1px solid #2d2d4a;border-radius:12px;padding:20px;min-width:240px;text-align:center;';
              popup.innerHTML =
                '<div style="color:#a0a0b8;font-size:13px;margin-bottom:12px;">选择标签颜色</div>' +
                '<input type="color" id="color-picker-popup" value="' + tag2.color + '" style="width:100%;height:120px;border:1px solid #2d2d4a;border-radius:8px;cursor:pointer;background:transparent;padding:2px;" />' +
                '<div id="color-picker-hex" style="color:#e8e8e8;font-size:13px;margin-top:8px;font-family:monospace;">' + tag2.color + '</div>' +
                '<div style="display:flex;gap:8px;margin-top:14px;justify-content:center;">' +
                  '<button id="color-picker-cancel" style="padding:8px 20px;background:#252540;color:#a0a0b8;border:1px solid #2d2d4a;border-radius:6px;cursor:pointer;font-size:13px;">取消</button>' +
                  '<button id="color-picker-confirm" style="padding:8px 20px;background:#6366f1;color:white;border:none;border-radius:6px;cursor:pointer;font-size:13px;">确认</button>' +
                '</div>';
              overlay.appendChild(popup);
              document.body.appendChild(overlay);

              var pickerInput = popup.querySelector('#color-picker-popup');
              var hexLabel = popup.querySelector('#color-picker-hex');
              // 实时显示色号
              pickerInput.addEventListener('input', function() {
                hexLabel.textContent = pickerInput.value;
              });
              // 取消
              popup.querySelector('#color-picker-cancel').addEventListener('click', function() {
                document.body.removeChild(overlay);
              });
              // 点遮罩也取消
              overlay.addEventListener('click', function(e) {
                if (e.target === overlay) document.body.removeChild(overlay);
              });
              // 确认
              popup.querySelector('#color-picker-confirm').addEventListener('click', async function() {
                var newColor = pickerInput.value;
                document.body.removeChild(overlay);
                try {
                  await invoke('update_tag', { tagId: tagContextTarget, name: null, color: newColor });
                  showToast('已改颜色');
                  fetchTags();
                } catch (e) { showToast('失败: ' + e, 'error'); }
              });
            }
            break;
          case 'delete-tag':
            var tagDelOk = await showConfirm('删除标签', '确定删除此标签？');
            if (tagDelOk) {
              try {
                await invoke('delete_tag', { tagId: tagContextTarget });
                showToast('已删除');
                // 从已选标签中移除被删除的标签
                state.selectedTagIds = state.selectedTagIds.filter(id => id !== tagContextTarget);
                fetchTags();
                fetchVideos();
              } catch (e) { showToast('失败: ' + e, 'error'); }
            }
            break;
        }
      };
    });
  }
}

// ============================================
// 标签右键菜单
// ============================================
var tagContextTarget = null;

async function init() {
  initContextMenu();
  bindEvents();
  await fetchTags();
  await fetchVideos();
  await fetchStats();
}

// 启动
document.addEventListener('DOMContentLoaded', init);
