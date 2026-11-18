<template>
  <section class="space-y-6">
    <!-- 顶部 3 大任务控制卡片 -->
    <div class="grid grid-cols-1 xl:grid-cols-3 gap-5">
      <!-- 1. TMDB 刮削调度器（支持暂停 / 继续 / 取消 / 重启） -->
      <div class="card p-6 rounded-2xl flex flex-col justify-between gap-4">
        <div class="space-y-3">
          <div class="flex items-center justify-between gap-2">
            <h2 class="text-base font-bold text-white flex items-center gap-2 min-w-0">
              <Sparkles class="w-4 h-4 text-emerald-400 shrink-0" aria-hidden="true" />
              <span class="truncate">TMDB 刮削任务调度</span>
            </h2>
            <span
              class="shrink-0 px-2.5 py-0.5 text-[11px] font-semibold rounded-full border"
              :class="scrapeBadgeClass"
            >
              {{ scrapeBadgeLabel }}
            </span>
          </div>

          <div class="space-y-1.5 text-xs text-slate-400">
            <div class="flex items-center justify-between gap-2 tabular-nums">
              <span class="truncate text-slate-200 font-medium">
                {{ scrapeStatus.task_name || (scrapeStatus.pending_remaining > 0 ? '待刮削队列等待中' : '当前无活跃刮削批次') }}
              </span>
              <span v-if="scrapeStatus.batch_total > 0" class="font-mono text-emerald-300 shrink-0">
                {{ scrapeStatus.batch_done }} / {{ scrapeStatus.batch_total }} ({{ scrapePercent }}%)
              </span>
            </div>
            <div class="w-full h-2 rounded-full bg-slate-900 border border-slate-800 overflow-hidden">
              <div
                v-if="scrapeStatus.batch_total > 0"
                class="h-full rounded-full transition-all duration-300"
                :class="scrapeStatus.status === 'paused' ? 'bg-amber-400' : 'bg-gradient-to-r from-emerald-500 to-cyan-400'"
                :style="{ width: `${scrapePercent}%` }"
              ></div>
              <div
                v-else-if="scrapeStatus.status === 'running'"
                class="h-full w-1/3 rounded-full bg-emerald-500/80 animate-pulse"
              ></div>
            </div>
            <div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] font-mono tabular-nums pt-0.5">
              <span class="text-slate-400">队列剩余 {{ scrapeStatus.pending_remaining || 0 }}</span>
              <span class="text-emerald-400">成功 {{ scrapeStatus.batch_succeeded || 0 }}</span>
              <span class="text-amber-400">失败/存疑 {{ scrapeStatus.batch_failed || 0 }}</span>
            </div>
            <p v-if="scrapeStatus.current_item" class="text-[11px] text-slate-400 truncate" :title="scrapeStatus.current_item">
              正在处理：{{ scrapeStatus.current_item }}
            </p>
          </div>
        </div>

        <div class="flex flex-wrap items-center gap-2 pt-1">
          <button
            v-if="scrapeStatus.status === 'running'"
            type="button"
            :disabled="busyScrapeAction !== null"
            @click="handlePauseScrape"
            class="cursor-pointer disabled:opacity-50 px-3.5 py-2 rounded-xl bg-amber-500/15 hover:bg-amber-500/25 border border-amber-500/35 text-amber-200 text-xs font-semibold flex items-center gap-1.5 transition-colors"
          >
            <Pause class="w-3.5 h-3.5" aria-hidden="true" />
            <span>暂停任务</span>
          </button>
          <button
            v-if="scrapeStatus.status === 'paused' || scrapeStatus.can_resume_auto"
            type="button"
            :disabled="busyScrapeAction !== null"
            @click="handleResumeScrape"
            class="cursor-pointer disabled:opacity-50 px-3.5 py-2 rounded-xl bg-emerald-600 hover:bg-emerald-500 text-white text-xs font-semibold flex items-center gap-1.5 transition-colors"
          >
            <Play class="w-3.5 h-3.5" aria-hidden="true" />
            <span>{{ scrapeStatus.status === 'paused' ? '继续任务' : '重新开始刮削' }}</span>
          </button>
          <button
            v-if="scrapeStatus.status === 'running' || scrapeStatus.status === 'paused'"
            type="button"
            :disabled="busyScrapeAction !== null"
            @click="handleCancelScrape"
            class="cursor-pointer disabled:opacity-50 px-3.5 py-2 rounded-xl bg-rose-500/15 hover:bg-rose-500/25 border border-rose-500/35 text-rose-200 text-xs font-semibold flex items-center gap-1.5 transition-colors"
          >
            <X class="w-3.5 h-3.5" aria-hidden="true" />
            <span>取消任务</span>
          </button>
          <button
            type="button"
            @click="loadAll"
            class="cursor-pointer px-3 py-2 rounded-xl bg-slate-800 hover:bg-slate-700 text-slate-200 text-xs font-medium flex items-center gap-1.5 transition-colors ml-auto"
          >
            <RefreshCw class="w-3.5 h-3.5 text-slate-300" aria-hidden="true" />
            <span>刷新</span>
          </button>
        </div>
      </div>

      <!-- 2. strm.txt / .zip 媒体库同步 -->
      <div class="card p-6 rounded-2xl flex flex-col justify-between gap-4">
        <div class="space-y-2.5">
          <div class="flex items-center justify-between gap-2">
            <h2 class="text-base font-bold text-white flex items-center gap-2 min-w-0">
              <Database class="w-4 h-4 text-sky-400 shrink-0" aria-hidden="true" />
              <span class="truncate">strm.txt / .zip 媒体库同步</span>
            </h2>
            <span
              class="shrink-0 whitespace-nowrap px-2.5 py-0.5 text-[11px] font-mono rounded-md border"
              :class="
                isSyncActive
                  ? 'bg-sky-950 text-sky-300 border-sky-500/40'
                  : 'bg-emerald-950 text-emerald-300 border-emerald-500/30'
              "
            >
              {{ isSyncActive ? `同步中 ${activeProgress?.percent ?? 5}%` : '增量对账' }}
            </span>
          </div>
          <p class="text-xs text-slate-400 leading-relaxed">
            对比本地 strm.txt 或远程 strm.zip 索引变更，完成新增入库、失效清理、多版本识别与熔断保护。
          </p>
          <div v-if="activeProgress" class="space-y-1.5 pt-1">
            <div class="flex items-center justify-between text-[11px] text-sky-300 font-mono tabular-nums">
              <span class="truncate">{{ activeProgress.message }}</span>
              <span>{{ activeProgress.percent }}%</span>
            </div>
            <div class="h-1.5 w-full bg-slate-900 rounded-full overflow-hidden border border-slate-800">
              <div
                class="h-full bg-gradient-to-r from-sky-500 via-cyan-400 to-emerald-400 rounded-full transition-all duration-300"
                :style="{ width: `${activeProgress.percent}%` }"
              ></div>
            </div>
          </div>
          <p v-else-if="logs.length > 0" class="text-[11px] text-slate-500 tabular-nums font-mono">
            上次同步：{{ formatDateTime(logs[0].started_at) }} · 扫描 {{ Number(logs[0].total_lines).toLocaleString() }} 行
          </p>
        </div>

        <div class="flex flex-wrap items-center gap-2.5 pt-1">
          <button
            type="button"
            @click="handleTriggerSync"
            :disabled="isSyncActive"
            class="cursor-pointer shrink-0 whitespace-nowrap bg-emerald-600 hover:bg-emerald-500 disabled:opacity-50 text-white px-4 py-2 rounded-xl text-xs font-semibold transition-colors flex items-center gap-1.5"
          >
            <Zap class="w-3.5 h-3.5 shrink-0" :class="{ 'animate-pulse': isSyncActive }" aria-hidden="true" />
            <span>{{ isSyncActive ? `同步中 (${activeProgress?.percent ?? 5}%)…` : '立即同步媒体库' }}</span>
          </button>
        </div>
      </div>

      <!-- 3. TMDB 扩展信息维护与失败重试 -->
      <div class="card p-6 rounded-2xl flex flex-col justify-between gap-4">
        <div class="space-y-2.5">
          <div class="flex items-center justify-between gap-2">
            <h2 class="text-base font-bold text-white flex items-center gap-2 min-w-0">
              <Layers class="w-4 h-4 text-cyan-400 shrink-0" aria-hidden="true" />
              <span class="truncate">TMDB 扩展信息维护</span>
            </h2>
            <span
              v-if="metadataStats"
              class="shrink-0 whitespace-nowrap px-2.5 py-0.5 text-[11px] font-mono rounded-md bg-slate-900 text-cyan-300 border border-slate-700/80 tabular-nums"
            >
              覆盖 {{ metadataPercent }}%
            </span>
          </div>
          <p class="text-xs text-slate-400 leading-relaxed">
            为已刮削影视补齐语言、投票数、时长与平台等扩展信息，并集中重排全部失败/存疑目录。
          </p>
          <p v-if="metadataStats" class="text-[11px] text-slate-500 tabular-nums font-mono">
            已补齐 {{ (metadataStats.enriched || 0).toLocaleString() }} / {{ (metadataStats.total || 0).toLocaleString() }} · 队列 {{ metadataStats.queued || 0 }} · 失败 {{ metadataStats.failed || 0 }}
          </p>
        </div>

        <div class="flex flex-wrap items-center gap-2.5 pt-1">
          <button
            type="button"
            @click="handleBackfill"
            :disabled="backfilling"
            class="cursor-pointer shrink-0 whitespace-nowrap bg-cyan-600 hover:bg-cyan-500 disabled:opacity-50 text-white px-4 py-2 rounded-xl text-xs font-semibold transition-colors flex items-center gap-1.5"
          >
            <Sparkles class="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
            <span>{{ backfilling ? '正在加入队列…' : '补齐 TMDB 扩展信息' }}</span>
          </button>
          <button
            type="button"
            @click="handleRetryFailed"
            :disabled="retrying"
            class="cursor-pointer shrink-0 whitespace-nowrap bg-slate-800 hover:bg-slate-700 disabled:opacity-50 px-4 py-2 rounded-xl text-xs font-medium text-slate-200 transition-colors flex items-center gap-1.5"
          >
            <RotateCcw class="w-3.5 h-3.5 text-amber-300 shrink-0" aria-hidden="true" />
            <span>{{ retrying ? '正在重排…' : '重试失败/存疑项' }}</span>
          </button>
        </div>
      </div>
    </div>

    <!-- 任务执行记录切换区 -->
    <div class="card rounded-2xl overflow-hidden">
      <div class="px-6 py-4 border-b border-slate-800 flex flex-wrap items-center justify-between gap-3">
        <div role="tablist" aria-label="任务执行记录分类" class="flex flex-wrap gap-2">
          <button
            type="button"
            role="tab"
            :aria-selected="recordTab === 'scrape'"
            @click="recordTab = 'scrape'"
            class="cursor-pointer px-3.5 py-1.5 rounded-xl text-xs font-semibold transition-colors flex items-center gap-1.5"
            :class="
              recordTab === 'scrape'
                ? 'bg-emerald-600 text-white'
                : 'bg-slate-900/90 text-slate-400 hover:text-slate-200 border border-slate-800'
            "
          >
            <Sparkles class="w-3.5 h-3.5" aria-hidden="true" />
            <span>TMDB 刮削任务记录 ({{ scrapeHistory.length }})</span>
          </button>
          <button
            type="button"
            role="tab"
            :aria-selected="recordTab === 'sync'"
            @click="recordTab = 'sync'"
            class="cursor-pointer px-3.5 py-1.5 rounded-xl text-xs font-semibold transition-colors flex items-center gap-1.5"
            :class="
              recordTab === 'sync'
                ? 'bg-emerald-600 text-white'
                : 'bg-slate-900/90 text-slate-400 hover:text-slate-200 border border-slate-800'
            "
          >
            <History class="w-3.5 h-3.5" aria-hidden="true" />
            <span>媒体库同步记录 ({{ displayedLogs.length }})</span>
          </button>
        </div>
        <span class="text-xs text-slate-400 whitespace-nowrap">按执行时间倒序排列</span>
      </div>

      <!-- 刮削任务记录表 -->
      <div v-if="recordTab === 'scrape'" tabindex="0" role="region" aria-label="TMDB 刮削任务记录" class="table-wrapper">
        <table class="w-full text-sm text-left">
          <thead class="bg-[#090d16] text-xs text-slate-400 border-b border-slate-800">
            <tr>
              <th scope="col" class="px-5 py-3.5 font-semibold whitespace-nowrap">任务序号</th>
              <th scope="col" class="px-4 py-3.5 font-semibold whitespace-nowrap">任务名称</th>
              <th scope="col" class="px-4 py-3.5 font-semibold whitespace-nowrap">开始时间</th>
              <th scope="col" class="px-4 py-3.5 font-semibold whitespace-nowrap">结束时间</th>
              <th scope="col" class="px-4 py-3.5 font-semibold whitespace-nowrap text-right">进度</th>
              <th scope="col" class="px-4 py-3.5 font-semibold whitespace-nowrap text-right">成功</th>
              <th scope="col" class="px-4 py-3.5 font-semibold whitespace-nowrap text-right">失败/存疑</th>
              <th scope="col" class="px-5 py-3.5 font-semibold whitespace-nowrap">状态</th>
            </tr>
          </thead>
          <tbody class="divide-y divide-slate-800/70">
            <tr v-if="scrapeHistory.length === 0">
              <td colspan="8" class="px-6 py-8 text-center text-xs text-slate-500">
                暂无本次运行周期内的刮削批次记录，触发批量刮削、重试或深度补齐后将自动记录在此
              </td>
            </tr>
            <tr v-for="rec in scrapeHistory" :key="rec.id" class="hover:bg-slate-900/40 transition-colors tabular-nums">
              <th scope="row" class="px-5 py-3.5 font-mono text-xs text-slate-400 font-normal whitespace-nowrap">
                #{{ rec.id }}
              </th>
              <td class="px-4 py-3.5 text-xs font-medium text-slate-100 whitespace-nowrap">{{ rec.task_name }}</td>
              <td class="px-4 py-3.5 font-mono text-xs text-slate-300 whitespace-nowrap">{{ formatDateTime(rec.started_at) }}</td>
              <td class="px-4 py-3.5 font-mono text-xs text-slate-400 whitespace-nowrap">
                {{ rec.finished_at ? formatDateTime(rec.finished_at) : '进行中…' }}
              </td>
              <td class="px-4 py-3.5 font-mono text-xs text-slate-200 whitespace-nowrap text-right">
                {{ rec.done }} / {{ rec.total }}
              </td>
              <td class="px-4 py-3.5 font-mono text-xs text-emerald-400 whitespace-nowrap text-right">
                {{ rec.succeeded }}
              </td>
              <td class="px-4 py-3.5 font-mono text-xs text-amber-400 whitespace-nowrap text-right">
                {{ rec.failed }}
              </td>
              <td class="px-5 py-3.5 text-xs whitespace-nowrap">
                <span class="px-2 py-0.5 rounded-md font-medium font-mono border" :class="scrapeHistoryClass(rec.status)">
                  {{ scrapeHistoryLabel(rec.status) }}
                </span>
              </td>
            </tr>
          </tbody>
        </table>
      </div>

      <!-- 媒体库同步记录表 -->
      <div v-else tabindex="0" role="region" aria-label="媒体库同步执行记录" class="table-wrapper">
        <table
          class="w-full text-sm text-left"
          style="--label-2: '启动时间'; --label-3: '总耗时'; --label-4: '扫描行数'; --label-5: '新增视频源'; --label-6: '移除失效源'; --label-7: '新增影视条目'; --label-8: '执行状态'"
        >
          <thead class="bg-[#090d16] text-xs text-slate-400 border-b border-slate-800">
            <tr>
              <th scope="col" class="px-5 py-3.5 font-semibold whitespace-nowrap">批次号</th>
              <th scope="col" class="px-4 py-3.5 font-semibold whitespace-nowrap">启动时间</th>
              <th scope="col" class="px-4 py-3.5 font-semibold whitespace-nowrap text-right">总耗时</th>
              <th scope="col" class="px-4 py-3.5 font-semibold whitespace-nowrap text-right">扫描行数</th>
              <th scope="col" class="px-4 py-3.5 font-semibold whitespace-nowrap text-right">新增视频源</th>
              <th scope="col" class="px-4 py-3.5 font-semibold whitespace-nowrap text-right">移除失效源</th>
              <th scope="col" class="px-4 py-3.5 font-semibold whitespace-nowrap text-right">新增影视条目</th>
              <th scope="col" class="px-5 py-3.5 font-semibold whitespace-nowrap">执行状态</th>
            </tr>
          </thead>
          <tbody class="divide-y divide-slate-800/70">
            <tr v-if="displayedLogs.length === 0">
              <td colspan="8" class="px-6 py-8 text-center text-xs text-slate-500">
                暂无同步记录，可点击上方「立即同步媒体库」开始首次同步
              </td>
            </tr>
            <tr v-for="l in displayedLogs" :key="l.id" class="hover:bg-slate-900/40 transition-colors tabular-nums">
              <th scope="row" class="px-5 py-3.5 font-mono text-xs text-slate-400 font-normal whitespace-nowrap">
                {{ l.id > 0 ? `#${l.id}` : '启动中' }}
              </th>
              <td class="px-4 py-3.5 text-xs text-slate-200 whitespace-nowrap font-mono">{{ formatDateTime(l.started_at) }}</td>
              <td class="px-4 py-3.5 font-mono text-xs text-slate-300 whitespace-nowrap text-right">
                {{ l.duration_ms != null ? `${l.duration_ms.toLocaleString()} ms` : '同步中…' }}
              </td>
              <td class="px-4 py-3.5 font-mono text-xs text-slate-200 whitespace-nowrap text-right">
                {{ Number(l.status === 'running' && activeProgress?.scanned_lines ? activeProgress.scanned_lines : l.total_lines).toLocaleString() }}
              </td>
              <td class="px-4 py-3.5 font-mono text-xs text-emerald-400 whitespace-nowrap text-right">+{{ Number(l.added_episodes).toLocaleString() }}</td>
              <td class="px-4 py-3.5 font-mono text-xs text-amber-400 whitespace-nowrap text-right">-{{ Number(l.deleted_episodes).toLocaleString() }}</td>
              <td class="px-4 py-3.5 font-mono text-xs text-cyan-400 whitespace-nowrap text-right">+{{ Number(l.new_media_items).toLocaleString() }}</td>
              <td class="px-5 py-3.5 text-xs">
                <div class="flex flex-col gap-1.5 min-w-[180px]">
                  <div class="flex items-center gap-2.5 min-w-0">
                    <span
                      class="shrink-0 whitespace-nowrap px-2 py-0.5 rounded-md font-medium font-mono"
                      :class="
                        l.status === 'success'
                          ? 'bg-emerald-950 text-emerald-300 border border-emerald-700/40'
                          : l.status === 'running'
                          ? 'bg-sky-950 text-sky-300 border border-sky-700/40'
                          : 'bg-rose-950 text-rose-300 border border-rose-700/40'
                      "
                    >
                      {{ l.status === 'running' && activeProgress ? `同步中 · ${activeProgress.percent}%` : syncStatusText(l.status) }}
                    </span>
                    <span
                      v-if="l.status === 'running' && activeProgress"
                      class="text-[11px] text-sky-300 truncate"
                      :title="activeProgress.message"
                    >
                      {{ activeProgress.message }}
                    </span>
                    <div
                      v-if="l.error_message"
                      tabindex="0"
                      :title="l.error_message"
                      class="single-line-reveal max-w-[260px] lg:max-w-[360px] xl:max-w-[460px] text-rose-300/95 bg-rose-950/35 border border-rose-800/40 px-2.5 py-1 rounded-lg font-mono text-[11px]"
                    >
                      <span>{{ l.error_message }}</span>
                    </div>
                  </div>
                  <div
                    v-if="l.status === 'running'"
                    class="h-1.5 w-full bg-slate-900 rounded-full overflow-hidden border border-slate-800"
                  >
                    <div
                      class="h-full bg-gradient-to-r from-sky-500 to-emerald-400 rounded-full transition-all duration-300"
                      :style="{ width: `${activeProgress?.percent ?? 10}%` }"
                    ></div>
                  </div>
                </div>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>
  </section>
</template>

<script setup>
import { ref, computed, onMounted, onUnmounted } from 'vue'
import {
  Database,
  Zap,
  RefreshCw,
  History,
  Sparkles,
  RotateCcw,
  Pause,
  Play,
  X,
  Layers
} from 'lucide-vue-next'
import { adminApi } from '../api/admin'
import { useToast, useConfirm } from '../composables/useToast'

const toast = useToast()
const confirmModal = useConfirm()

const recordTab = ref('scrape')
const logs = ref([])
const activeProgress = ref(null)
const metadataStats = ref(null)
const scrapeStatus = ref({
  status: 'idle',
  running: false,
  paused: false,
  can_resume_auto: false,
  task_name: null,
  current_item: null,
  batch_total: 0,
  batch_done: 0,
  batch_succeeded: 0,
  batch_failed: 0,
  pending_remaining: 0,
  percent: null,
  history: []
})

const syncing = ref(false)
const retrying = ref(false)
const backfilling = ref(false)
const busyScrapeAction = ref(null)
let pollTimer = null

const isSyncActive = computed(() => {
  return syncing.value || activeProgress.value !== null || logs.value.some(l => l.status === 'running')
})

const displayedLogs = computed(() => {
  if (!activeProgress.value) return logs.value
  const hasRunning = logs.value.some(l => l.status === 'running')
  if (hasRunning) return logs.value
  return [
    {
      id: 0,
      started_at: new Date().toISOString(),
      duration_ms: null,
      total_lines: activeProgress.value.scanned_lines || 0,
      added_episodes: 0,
      deleted_episodes: 0,
      new_media_items: 0,
      status: 'running',
      error_message: ''
    },
    ...logs.value
  ]
})

const scrapeHistory = computed(() => scrapeStatus.value.history || [])

const scrapePercent = computed(() => {
  if (typeof scrapeStatus.value.percent === 'number') {
    return Math.min(100, Math.max(0, scrapeStatus.value.percent))
  }
  if (scrapeStatus.value.batch_total > 0) {
    return Math.round((scrapeStatus.value.batch_done / scrapeStatus.value.batch_total) * 1000) / 10
  }
  return 0
})

const scrapeBadgeLabel = computed(() => {
  if (scrapeStatus.value.status === 'running') return '运行中'
  if (scrapeStatus.value.status === 'paused') return '已暂停'
  if (scrapeStatus.value.can_resume_auto) return '已中止'
  return '空闲'
})

const scrapeBadgeClass = computed(() => {
  if (scrapeStatus.value.status === 'running') {
    return 'bg-emerald-500/15 text-emerald-300 border-emerald-500/35'
  }
  if (scrapeStatus.value.status === 'paused') {
    return 'bg-amber-500/15 text-amber-300 border-amber-500/35'
  }
  if (scrapeStatus.value.can_resume_auto) {
    return 'bg-rose-500/15 text-rose-300 border-rose-500/35'
  }
  return 'bg-slate-800 text-slate-300 border-slate-700'
})

const metadataPercent = computed(() => {
  const s = metadataStats.value
  if (!s || !s.total) return 0
  return Math.min(100, Math.round(((s.enriched || 0) / s.total) * 100))
})

async function loadAll(silent = false) {
  try {
    const [logData, scrapeData] = await Promise.all([
      adminApi.listSyncLogs(),
      adminApi.getScrapeStatus().catch(() => null)
    ])
    logs.value = logData.logs || []
    activeProgress.value = logData.active_progress || logData.progress || null
    if (logData.metadata) {
      metadataStats.value = logData.metadata
    }
    if (scrapeData) {
      scrapeStatus.value = scrapeData
    }
    if (activeProgress.value || logs.value.some(l => l.status === 'running')) {
      syncing.value = true
      ensurePolling()
    } else {
      syncing.value = false
      if (scrapeStatus.value.status === 'running' || scrapeStatus.value.status === 'paused') {
        ensurePolling()
      } else {
        stopPolling()
      }
    }
  } catch (err) {
    if (!silent) toast.error('加载任务状态失败：' + err.message)
  }
}

function ensurePolling() {
  if (pollTimer) return
  pollTimer = setInterval(() => {
    loadAll(true)
  }, 1800)
}

function stopPolling() {
  if (pollTimer) {
    clearInterval(pollTimer)
    pollTimer = null
  }
}

async function handlePauseScrape() {
  if (busyScrapeAction.value) return
  busyScrapeAction.value = 'pause'
  try {
    const res = await adminApi.pauseScrapeTask()
    scrapeStatus.value = res
    toast.info(res.message || '已暂停刮削任务')
    window.dispatchEvent(new CustomEvent('rx-scrape-task-started'))
  } catch (err) {
    toast.error('暂停失败：' + err.message)
  } finally {
    busyScrapeAction.value = null
  }
}

async function handleResumeScrape() {
  if (busyScrapeAction.value) return
  busyScrapeAction.value = 'resume'
  try {
    const res = await adminApi.resumeScrapeTask()
    scrapeStatus.value = res
    toast.success(res.message || '已恢复刮削任务')
    ensurePolling()
    window.dispatchEvent(new CustomEvent('rx-scrape-task-started'))
  } catch (err) {
    toast.error('恢复失败：' + err.message)
  } finally {
    busyScrapeAction.value = null
  }
}

async function handleCancelScrape() {
  if (busyScrapeAction.value) return
  const ok = await confirmModal({
    title: '取消刮削任务',
    message: '确定要取消当前刮削任务吗？已完成的条目会保留，未处理的条目将停止拉取。',
    confirmText: '确认取消',
    danger: true
  })
  if (!ok) return
  busyScrapeAction.value = 'cancel'
  try {
    const res = await adminApi.cancelScrapeTask()
    scrapeStatus.value = res
    toast.info(res.message || '已取消当前刮削队列')
    window.dispatchEvent(new CustomEvent('rx-scrape-task-started'))
  } catch (err) {
    toast.error('取消失败：' + err.message)
  } finally {
    busyScrapeAction.value = null
  }
}

async function handleTriggerSync() {
  if (isSyncActive.value) return
  syncing.value = true
  try {
    const data = await adminApi.triggerSync()
    toast.info(data.message || '已触发同步，正在后台对账')
    ensurePolling()
    await loadAll(true)
  } catch (err) {
    toast.error('触发同步失败：' + err.message)
    syncing.value = false
  }
}

async function handleRetryFailed() {
  if (retrying.value) return
  retrying.value = true
  try {
    const res = await adminApi.retryFailedScrapes()
    toast.success(res.message)
    ensurePolling()
    window.dispatchEvent(new CustomEvent('rx-scrape-task-started'))
    await loadAll(true)
  } catch (err) {
    toast.error('重试失败：' + err.message)
  } finally {
    retrying.value = false
  }
}

async function handleBackfill() {
  if (backfilling.value) return
  const ok = await confirmModal({
    title: '补齐 TMDB 扩展信息',
    message: '要为本地已刮削且未锁定的影视补齐语言、投票数、时长等扩展信息吗？任务会分批请求 TMDB，可在关闭全库自动刮削时执行。',
    confirmText: '开始补齐'
  })
  if (!ok) return
  backfilling.value = true
  try {
    const res = await adminApi.backfillSmartViewMetadata()
    toast.success(res.message)
    ensurePolling()
    window.dispatchEvent(new CustomEvent('rx-scrape-task-started'))
    await loadAll(true)
  } catch (err) {
    toast.error('补齐失败：' + err.message)
  } finally {
    backfilling.value = false
  }
}

function syncStatusText(status) {
  if (status === 'success') return '已完成'
  if (status === 'running') return '同步中'
  if (status === 'failed') return '失败'
  return status || '未知'
}

function scrapeHistoryLabel(status) {
  if (status === 'running') return '进行中'
  if (status === 'paused') return '已暂停'
  if (status === 'completed') return '已完成'
  if (status === 'cancelled') return '已取消'
  return status || '未知'
}

function scrapeHistoryClass(status) {
  if (status === 'completed') return 'bg-emerald-950 text-emerald-300 border-emerald-700/40'
  if (status === 'running') return 'bg-sky-950 text-sky-300 border-sky-700/40'
  if (status === 'paused') return 'bg-amber-950 text-amber-300 border-amber-700/40'
  if (status === 'cancelled') return 'bg-rose-950 text-rose-300 border-rose-700/40'
  return 'bg-slate-800 text-slate-300 border-slate-700'
}

function formatDateTime(iso) {
  if (!iso) return '-'
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return iso
  return d.toLocaleString('zh-CN', { hour12: false })
}

onMounted(() => loadAll())
onUnmounted(stopPolling)
</script>
