const TOKEN_KEY = 'rx_emby_admin_token'
let unauthorizedHandler = null

export function getAdminToken() {
  try {
    return localStorage.getItem(TOKEN_KEY) || ''
  } catch {
    return ''
  }
}

export function setAdminToken(token) {
  try {
    localStorage.setItem(TOKEN_KEY, token)
  } catch {
    // 忽略隐私模式存储限制
  }
}

export function embyImageUrl(path) {
  const token = getAdminToken()
  if (!token) return path
  const sep = path.includes('?') ? '&' : '?'
  return `${path}${sep}api_key=${encodeURIComponent(token)}`
}

export function clearAdminToken() {
  try {
    localStorage.removeItem(TOKEN_KEY)
  } catch {
    // 忽略隐私模式存储限制
  }
}

export function setUnauthorizedHandler(handler) {
  unauthorizedHandler = handler
}

async function requestJson(url, options = {}) {
  const token = getAdminToken()
  const headers = new Headers(options.headers || {})
  if (token) {
    headers.set('Authorization', `Bearer ${token}`)
  }

  const res = await fetch(url, { ...options, headers })
  if (!res.ok) {
    const errText = await res.text()
    if ((res.status === 401 || res.status === 403) && !url.startsWith('/api/admin/auth/login')) {
      clearAdminToken()
      if (unauthorizedHandler) {
        unauthorizedHandler(errText || '登录已失效，请重新登录')
      }
    }
    throw new Error(errText || `HTTP ${res.status}`)
  }
  return res.json()
}

export const adminApi = {
  // 鉴权与会话
  login(username, password) {
    return requestJson('/api/admin/auth/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ username, password })
    })
  },
  logout() {
    return requestJson('/api/admin/auth/logout', { method: 'POST' })
  },
  getMe() {
    return requestJson('/api/admin/auth/me')
  },

  // 0. 全局概览仪表盘
  getOverview() {
    return requestJson('/api/admin/overview')
  },
  getResourceMetrics() {
    return requestJson('/api/admin/resources')
  },
  sampleResourceMetrics() {
    return requestJson('/api/admin/resources/sample', { method: 'POST' })
  },

  // 1. 海报墙与刮削纠偏
  listMedia(params) {
    const qs = new URLSearchParams(params).toString()
    return requestJson(`/api/admin/media?${qs}`)
  },
  getMediaDetail(id) {
    return requestJson(`/api/admin/media/${id}/detail`)
  },
  prewarmMediaDirs(id) {
    return requestJson(`/api/admin/media/${id}/prewarm_dirs`, { method: 'POST' })
  },
  scrapeOne(id) {
    return requestJson(`/api/admin/media/${id}/scrape`, { method: 'POST' })
  },
  toggleIgnore(id, ignore) {
    return requestJson(`/api/admin/media/${id}/ignore`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ ignore })
    })
  },
  bindTmdb(id, payload) {
    return requestJson(`/api/admin/media/${id}/bind_tmdb`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload)
    })
  },
  updateMediaMetadata(id, payload) {
    return requestJson(`/api/admin/media/${id}/metadata`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload)
    })
  },
  unbindMediaTmdb(id, payload = {}) {
    return requestJson(`/api/admin/media/${id}/unbind_tmdb`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload)
    })
  },
  splitFolder(folderId) {
    return requestJson(`/api/admin/folders/${folderId}/split`, {
      method: 'POST'
    })
  },
  updateFileEpisode(fileId, payload) {
    return requestJson(`/api/admin/files/${fileId}/episode`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload)
    })
  },
  splitWorkEpisodes(workId, payload) {
    return requestJson(`/api/admin/media/${workId}/split_episodes`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload)
    })
  },
  previewReparseStrm(workId) {
    return requestJson(`/api/admin/media/${workId}/reparse_strm/preview`, {
      method: 'POST'
    })
  },
  applyReparseStrm(workId, payload) {
    return requestJson(`/api/admin/media/${workId}/reparse_strm/apply`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload)
    })
  },
  retryFailedScrapes() {
    return requestJson('/api/admin/staging/retry_failed', { method: 'POST' })
  },
  getScrapeStatus() {
    return requestJson('/api/admin/scrape/status')
  },
  pauseScrapeTask() {
    return requestJson('/api/admin/scrape/pause', { method: 'POST' })
  },
  resumeScrapeTask() {
    return requestJson('/api/admin/scrape/resume', { method: 'POST' })
  },
  cancelScrapeTask() {
    return requestJson('/api/admin/scrape/cancel', { method: 'POST' })
  },
  updateFolderPriority(folderId, priorityBoost) {
    return requestJson(`/api/admin/folders/${folderId}/priority`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ priority_boost: priorityBoost })
    })
  },
  updateFilePriority(fileId, priorityOverride) {
    return requestJson(`/api/admin/files/${fileId}/priority`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ priority_override: priorityOverride })
    })
  },

  // 1b. 中间表 (Staging) 工作台
  listStagingFolders(params) {
    const qs = new URLSearchParams(params).toString()
    return requestJson(`/api/admin/staging/folders?${qs}`)
  },
  listStagingFolderFiles(folderId) {
    return requestJson(`/api/admin/staging/folders/${folderId}/files`)
  },
  scrapeStagingFolder(folderId) {
    return requestJson(`/api/admin/staging/folders/${folderId}/scrape`, { method: 'POST' })
  },
  promoteStagingFolder(folderId, payload) {
    return requestJson(`/api/admin/staging/folders/${folderId}/promote`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload)
    })
  },
  toggleIgnoreStagingFolder(folderId, ignore) {
    return requestJson(`/api/admin/staging/folders/${folderId}/ignore`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ ignore })
    })
  },
  pauseStagingFolder(folderId, paused) {
    return requestJson(`/api/admin/staging/folders/${folderId}/pause`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ paused })
    })
  },
  batchPauseStagingFolders(folderIds, paused) {
    return requestJson('/api/admin/staging/pause_batch', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ folder_ids: folderIds, paused })
    })
  },
  batchScrapeStagingFolders(folderIds) {
    return requestJson('/api/admin/staging/scrape_batch', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ folder_ids: folderIds })
    })
  },

  // 2. 自定义虚拟媒体库 (Smart Views)
  listVirtualLibraries() {
    return requestJson('/api/admin/virtual_libraries')
  },
  smartViewOptions() { return requestJson('/api/admin/virtual_libraries/options') },
  previewSmartView(rules) {
    return requestJson('/api/admin/virtual_libraries/preview', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(rules) })
  },
  backfillSmartViewMetadata() { return requestJson('/api/admin/virtual_libraries/backfill', { method: 'POST' }) },
  saveVirtualLibrary(payload) {
    return requestJson('/api/admin/virtual_libraries', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload)
    })
  },
  async exportVirtualLibrariesBundle() {
    const token = getAdminToken()
    const headers = new Headers()
    if (token) {
      headers.set('Authorization', `Bearer ${token}`)
    }
    const res = await fetch('/api/admin/virtual_libraries/export', { headers })
    if (!res.ok) {
      const errText = await res.text()
      if (res.status === 401 || res.status === 403) {
        clearAdminToken()
        if (unauthorizedHandler) {
          unauthorizedHandler(errText || '登录已失效，请重新登录')
        }
      }
      throw new Error(errText || `HTTP ${res.status}`)
    }
    return res.blob()
  },
  previewImportVirtualLibraries(bytes) {
    return requestJson('/api/admin/virtual_libraries/import/preview', {
      method: 'POST',
      headers: { 'Content-Type': 'application/octet-stream' },
      body: bytes
    })
  },
  applyImportVirtualLibraries(bytes, pruneMissing = false) {
    const qs = pruneMissing ? '?prune_missing=true' : '?prune_missing=false'
    return requestJson(`/api/admin/virtual_libraries/import/apply${qs}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/octet-stream' },
      body: bytes
    })
  },
  deleteVirtualLibrary(id) {
    return requestJson(`/api/admin/virtual_libraries/${id}`, { method: 'DELETE' })
  },
  listBaseLibraryCovers() {
    return requestJson('/api/admin/base_libraries/covers')
  },
  saveBaseLibraryCover(payload) {
    return requestJson('/api/admin/base_libraries/covers', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload)
    })
  },
  refreshDynamicCovers() {
    return requestJson('/api/admin/covers/refresh', { method: 'POST' })
  },

  // 3. 多用户账号、分类权限与在线会话
  listUsersAndSessions() {
    return requestJson('/api/admin/users')
  },
  saveUser(payload) {
    return requestJson('/api/admin/users', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload)
    })
  },
  deleteUser(uid) {
    return requestJson(`/api/admin/users/${encodeURIComponent(uid)}`, { method: 'DELETE' })
  },
  revokeToken(token) {
    return requestJson(`/api/admin/tokens/${encodeURIComponent(token)}`, { method: 'DELETE' })
  },

  // 4. 增量同步与图片 LRU 缓存
  getCacheStats() {
    return requestJson('/api/admin/cache/stats')
  },
  triggerCacheEvict() {
    return requestJson('/api/admin/cache/evict', { method: 'POST' })
  },
  listSyncLogs() {
    return requestJson('/api/admin/sync/logs')
  },
  triggerSync() {
    return requestJson('/api/admin/sync/trigger', { method: 'POST' })
  },

  // 5. 全局动态热配置
  listConfigs() {
    return requestJson('/api/admin/configs')
  },
  updateConfigs(updates) {
    return requestJson('/api/admin/configs', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ updates })
    })
  }
}
