import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import { execSync } from 'node:child_process'
import { readFileSync } from 'node:fs'

function runGit(cmd) {
  try {
    return execSync(cmd, { stdio: ['ignore', 'pipe', 'ignore'] }).toString().trim()
  } catch {
    return ''
  }
}

function normalizeRepoUrl(raw) {
  if (!raw) return ''
  const trimmed = raw.trim().replace(/\.git$/, '')
  if (trimmed.startsWith('git@github.com:')) {
    return `https://github.com/${trimmed.slice('git@github.com:'.length)}`
  }
  if (trimmed.startsWith('https://github.com/') || trimmed.startsWith('http://github.com/')) {
    return trimmed.replace(/^http:\/\//, 'https://')
  }
  return ''
}

function resolveBuildInfo() {
  const pkg = JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8'))
  const rawVersion = (process.env.APP_VERSION || pkg.version || '0.0.0').replace(/^v/, '')
  const envSha = (process.env.GIT_SHA || process.env.GITHUB_SHA || '').trim()
  const gitFullSha = envSha || runGit('git rev-parse HEAD')
  const gitSha = gitFullSha ? gitFullSha.slice(0, 7) : 'unknown'
  const gitBranch = (process.env.GIT_BRANCH || process.env.GITHUB_REF_NAME || '').trim() || runGit('git rev-parse --abbrev-ref HEAD')
  const gitDirty = !envSha && Boolean(runGit('git status --porcelain'))
  const buildTime = (process.env.BUILD_TIME || '').trim() || new Date().toISOString()
  const envRepo = (process.env.GITHUB_REPOSITORY || '').trim()
  const repoUrl = envRepo
    ? `https://github.com/${envRepo}`
    : normalizeRepoUrl(runGit('git remote get-url origin')) || 'https://github.com/Clowrain/rx-emby'

  return {
    version: rawVersion,
    gitSha,
    gitFullSha,
    gitBranch,
    gitDirty,
    buildTime,
    repoUrl
  }
}

export default defineConfig({
  plugins: [vue()],
  define: {
    __APP_BUILD_INFO__: JSON.stringify(resolveBuildInfo())
  },
  base: '/',
  build: {
    outDir: '../web_ui',
    emptyOutDir: true,
    assetsDir: 'assets'
  },
  server: {
    port: 5173,
    proxy: {
      '/api': 'http://127.0.0.1:8096',
      '/emby': 'http://127.0.0.1:8096'
    }
  }
})
