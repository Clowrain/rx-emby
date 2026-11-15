<template>
  <section class="space-y-6">
    <!-- 顶部二级导航 Tab -->
    <div class="card p-3 rounded-2xl flex flex-wrap items-center justify-between gap-3">
      <div role="tablist" aria-label="同步与刮削功能" class="flex flex-wrap gap-2">
        <button
          type="button"
          role="tab"
          :aria-selected="activeTab === 'tasks'"
          @click="activeTab = 'tasks'"
          class="cursor-pointer px-4 py-2 rounded-xl text-xs font-semibold transition-colors flex items-center gap-2"
          :class="
            activeTab === 'tasks'
              ? 'bg-emerald-600 text-white shadow-sm'
              : 'bg-slate-900/90 text-slate-400 hover:text-slate-200 border border-slate-800'
          "
        >
          <Database class="w-3.5 h-3.5" aria-hidden="true" />
          <span>同步与刮削任务</span>
        </button>
        <button
          type="button"
          role="tab"
          :aria-selected="activeTab === 'config'"
          @click="activeTab = 'config'"
          class="cursor-pointer px-4 py-2 rounded-xl text-xs font-semibold transition-colors flex items-center gap-2"
          :class="
            activeTab === 'config'
              ? 'bg-emerald-600 text-white shadow-sm'
              : 'bg-slate-900/90 text-slate-400 hover:text-slate-200 border border-slate-800'
          "
        >
          <SlidersHorizontal class="w-3.5 h-3.5" aria-hidden="true" />
          <span>同步策略与源过滤</span>
        </button>
      </div>
    </div>

    <!-- Tab 1: 同步与刮削任务 + 同步流水 -->
    <template v-if="activeTab === 'tasks'">
      <div class="grid grid-cols-1 lg:grid-cols-2 gap-6">
        <!-- strm.txt 增量同步卡片 -->
        <div class="card p-6 rounded-2xl flex flex-col justify-between gap-5">
          <div class="space-y-2.5">
            <div class="flex items-center justify-between gap-3">
              <h2 class="text-base font-bold text-white flex items-center gap-2 min-w-0">
                <Database class="w-4 h-4 text-emerald-400 shrink-0" aria-hidden="true" />
                <span class="truncate">strm.txt 媒体库同步</span>
              </h2>
              <span class="shrink-0 whitespace-nowrap px-2.5 py-0.5 text-[11px] font-mono rounded-md bg-emerald-950 text-emerald-300 border border-emerald-500/30">
                增量对账
              </span>
            </div>
            <p class="text-xs text-slate-400 leading-relaxed">
              对比 strm.txt 索引变更，自动完成新增入库、失效片源清理、同剧集跨目录合并及画质版本识别。
            </p>
          </div>

          <div class="flex flex-wrap items-center gap-3 pt-1">
            <button
              type="button"
              @click="handleTriggerSync"
              :disabled="syncing"
              class="cursor-pointer shrink-0 whitespace-nowrap bg-emerald-600 hover:bg-emerald-500 disabled:opacity-50 text-white px-5 py-2.5 rounded-xl text-xs font-semibold transition-colors flex items-center gap-1.5"
            >
              <Zap class="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
              <span>{{ syncing ? '正在启动同步…' : '立即同步媒体库' }}</span>
            </button>
            <button
              type="button"
              @click="loadAll"
              class="cursor-pointer shrink-0 whitespace-nowrap bg-slate-800 hover:bg-slate-700 px-4 py-2.5 rounded-xl text-xs font-medium text-slate-200 transition-colors flex items-center gap-1.5"
            >
              <RefreshCw class="w-3.5 h-3.5 text-slate-300 shrink-0" aria-hidden="true" />
              <span>刷新记录</span>
            </button>
          </div>
        </div>

        <!-- 刮削与元数据维护卡片 -->
        <div class="card p-6 rounded-2xl flex flex-col justify-between gap-5">
          <div class="space-y-2.5">
            <div class="flex items-center justify-between gap-3">
              <h2 class="text-base font-bold text-white flex items-center gap-2 min-w-0">
                <Sparkles class="w-4 h-4 text-cyan-400 shrink-0" aria-hidden="true" />
                <span class="truncate">TMDB 刮削与扩展信息维护</span>
              </h2>
              <span
                v-if="metadataStats"
                class="shrink-0 whitespace-nowrap px-2.5 py-0.5 text-[11px] font-mono rounded-md bg-slate-900 text-cyan-300 border border-slate-700/80 tabular-nums"
              >
                扩展覆盖 {{ metadataPercent }}%
              </span>
            </div>
            <p class="text-xs text-slate-400 leading-relaxed">
              集中处理失败刮削重试，以及为已刮削影视补齐语言、投票数、时长与平台等 TMDB 扩展信息（供智能视图高级规则使用）。
            </p>
            <p v-if="metadataStats" class="text-[11px] text-slate-500 tabular-nums">
              已补齐 {{ (metadataStats.enriched || 0).toLocaleString() }} / {{ (metadataStats.total || 0).toLocaleString() }} 部 · 队列中 {{ metadataStats.queued || 0 }} 部 · 失败 {{ metadataStats.failed || 0 }} 部
            </p>
          </div>

          <div class="flex flex-wrap items-center gap-3 pt-1">
            <button
              type="button"
              @click="handleBackfill"
              :disabled="backfilling"
              class="cursor-pointer shrink-0 whitespace-nowrap bg-cyan-600 hover:bg-cyan-500 disabled:opacity-50 text-white px-4 py-2.5 rounded-xl text-xs font-semibold transition-colors flex items-center gap-1.5"
            >
              <Sparkles class="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
              <span>{{ backfilling ? '正在加入队列…' : '补齐 TMDB 扩展信息' }}</span>
            </button>
            <button
              type="button"
              @click="handleRetryFailed"
              :disabled="retrying"
              class="cursor-pointer shrink-0 whitespace-nowrap bg-slate-800 hover:bg-slate-700 disabled:opacity-50 px-4 py-2.5 rounded-xl text-xs font-medium text-slate-200 transition-colors flex items-center gap-1.5"
            >
              <RotateCcw class="w-3.5 h-3.5 text-amber-300 shrink-0" aria-hidden="true" />
              <span>{{ retrying ? '正在重排…' : '重试失败刮削' }}</span>
            </button>
          </div>
        </div>
      </div>

      <!-- 同步流水日志表格 -->
      <div class="card rounded-2xl overflow-hidden">
        <div class="px-6 py-4 border-b border-slate-800 flex items-center justify-between gap-3">
          <h2 id="sync-logs-heading" class="text-base font-bold text-white flex items-center gap-2">
            <History class="w-4 h-4 text-emerald-400 shrink-0" aria-hidden="true" />
            <span>最近 30 次同步记录</span>
          </h2>
          <span class="text-xs text-slate-400 whitespace-nowrap">按执行时间倒序排列</span>
        </div>
        <div tabindex="0" role="region" aria-labelledby="sync-logs-heading" class="table-wrapper">
          <table
            class="w-full text-sm text-left"
            style="--label-2: '启动时间'; --label-3: '总耗时'; --label-4: '扫描行数'; --label-5: '新增视频源'; --label-6: '移除失效源'; --label-7: '新增影视条目'; --label-8: '执行状态'"
          >
            <caption class="visually-hidden">最近 30 次同步执行记录</caption>
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
              <tr v-if="logs.length === 0">
                <td colspan="8" class="px-6 py-8 text-center text-xs text-slate-500">
                  暂无同步记录，可点击上方「立即同步媒体库」开始首次同步
                </td>
              </tr>
              <tr v-for="l in logs" :key="l.id" class="hover:bg-slate-900/40 transition-colors tabular-nums">
                <th scope="row" class="px-5 py-3.5 font-mono text-xs text-slate-400 font-normal whitespace-nowrap">#{{ l.id }}</th>
                <td class="px-4 py-3.5 text-xs text-slate-200 whitespace-nowrap font-mono">{{ formatDateTime(l.started_at) }}</td>
                <td class="px-4 py-3.5 font-mono text-xs text-slate-300 whitespace-nowrap text-right">
                  {{ l.duration_ms != null ? `${l.duration_ms.toLocaleString()} ms` : '同步中…' }}
                </td>
                <td class="px-4 py-3.5 font-mono text-xs text-slate-200 whitespace-nowrap text-right">{{ Number(l.total_lines).toLocaleString() }}</td>
                <td class="px-4 py-3.5 font-mono text-xs text-emerald-400 whitespace-nowrap text-right">+{{ Number(l.added_episodes).toLocaleString() }}</td>
                <td class="px-4 py-3.5 font-mono text-xs text-amber-400 whitespace-nowrap text-right">-{{ Number(l.deleted_episodes).toLocaleString() }}</td>
                <td class="px-4 py-3.5 font-mono text-xs text-cyan-400 whitespace-nowrap text-right">+{{ Number(l.new_media_items).toLocaleString() }}</td>
                <td class="px-5 py-3.5 text-xs">
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
                      {{ syncStatusText(l.status) }}
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
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>
    </template>

    <!-- Tab 2: 同步策略与源过滤配置 -->
    <ConfigGroupPanel
      v-else
      prefix="sync."
      title="strm.txt 同步策略与源过滤"
      description="配置 strm.txt 索引源路径、定时同步周期、目录白名单前缀、排除关键词与调试行数上限，保存后立即生效。"
      :icon="SlidersHorizontal"
      @saved="$emit('saved')"
    />
  </section>
</template>

<script setup>
import { ref, computed, onMounted } from 'vue'
import { Database, Zap, RefreshCw, History, Sparkles, RotateCcw, SlidersHorizontal } from 'lucide-vue-next'
import { adminApi } from '../api/admin'
import { useToast, useConfirm } from '../composables/useToast'
import ConfigGroupPanel from './ConfigGroupPanel.vue'

defineEmits(['saved'])

const toast = useToast()
const confirmModal = useConfirm()
const activeTab = ref('tasks')
const logs = ref([])
const metadataStats = ref(null)
const syncing = ref(false)
const retrying = ref(false)
const backfilling = ref(false)

const metadataPercent = computed(() => {
  const total = Number(metadataStats.value?.total || 0)
  const enriched = Number(metadataStats.value?.enriched || 0)
  if (total <= 0) return 0
  return Math.min(100, Math.round((enriched / total) * 100))
})

async function loadAll() {
  try {
    const logsData = await adminApi.listSyncLogs()
    logs.value = logsData.logs || []
    if (logsData.metadata) {
      metadataStats.value = logsData.metadata
    } else {
      adminApi.smartViewOptions()
        .then(opts => {
          if (opts?.metadata) metadataStats.value = opts.metadata
        })
        .catch(() => null)
    }
  } catch (err) {
    toast.error('加载同步记录失败：' + err.message)
  }
}

async function handleTriggerSync() {
  syncing.value = true
  try {
    const data = await adminApi.triggerSync()
    toast.success(data.message)
    setTimeout(() => {
      syncing.value = false
      loadAll()
    }, 1200)
  } catch (err) {
    syncing.value = false
    toast.error('触发同步失败：' + err.message)
  }
}

async function handleRetryFailed() {
  if (retrying.value) return
  retrying.value = true
  try {
    const res = await adminApi.retryFailedScrapes()
    toast.success(res.message)
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
    await loadAll()
  } catch (err) {
    toast.error('补齐失败：' + err.message)
  } finally {
    backfilling.value = false
  }
}

function syncStatusText(status) {
  return { success: '成功', running: '同步中', failed: '失败' }[status] || status
}

function formatDateTime(ts) {
  if (!ts) return '-'
  const d = new Date(ts)
  if (Number.isNaN(d.getTime())) return String(ts)
  const pad = n => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
}

onMounted(loadAll)
</script>
