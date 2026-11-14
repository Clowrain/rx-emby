/**
 * 格式化构建时间（精确到分钟），无效时间返回空字符串
 */
export function formatBuildTime(isoString) {
  if (!isoString) return ''
  const date = new Date(isoString)
  if (Number.isNaN(date.getTime())) return ''
  const pad = n => String(n).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`
}

/**
 * 读取 Vite 编译期注入的构建版本信息
 */
export function getBuildInfo(raw = typeof __APP_BUILD_INFO__ !== 'undefined' ? __APP_BUILD_INFO__ : {}) {
  const version = raw?.version ? `v${String(raw.version).replace(/^v/, '')}` : 'v0.0.0'
  const gitSha = raw?.gitSha || 'unknown'
  const gitBranch = raw?.gitBranch || ''
  const gitDirty = Boolean(raw?.gitDirty)
  const buildTime = formatBuildTime(raw?.buildTime)

  const commitLabel = gitDirty ? `${gitSha}-dirty` : gitSha
  const parts = [`RX-Emby ${version}`, commitLabel]
  if (gitBranch && gitBranch !== 'HEAD') parts.push(gitBranch)
  if (buildTime) parts.push(`built ${buildTime}`)

  return {
    version,
    gitSha,
    gitBranch,
    gitDirty,
    commitLabel,
    buildTime,
    diagnosticText: parts.join(' · ')
  }
}
