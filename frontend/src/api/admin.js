async function requestJson(url, options = {}) {
  const res = await fetch(url, options)
  if (!res.ok) {
    const errText = await res.text()
    throw new Error(errText || `HTTP ${res.status}`)
  }
  return res.json()
}

export const adminApi = {
  // 0. 全局概览仪表盘
  getOverview() {
    return requestJson('/api/admin/overview')
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
  retryFailedScrapes() {
    return requestJson('/api/admin/scrape/retry_failed', { method: 'POST' })
  },

  // 2. 自定义虚拟媒体库 (Smart Views)
  listVirtualLibraries() {
    return requestJson('/api/admin/virtual_libraries')
  },
  saveVirtualLibrary(payload) {
    return requestJson('/api/admin/virtual_libraries', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload)
    })
  },
  seedPresetVirtualLibraries() {
    return requestJson('/api/admin/virtual_libraries/presets', { method: 'POST' })
  },
  deleteVirtualLibrary(id) {
    return requestJson(`/api/admin/virtual_libraries/${id}`, { method: 'DELETE' })
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
