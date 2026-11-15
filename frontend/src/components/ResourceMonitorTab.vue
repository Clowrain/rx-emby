<script setup>
import { ref, computed, onMounted, onUnmounted } from 'vue'
import {
  Cpu,
  Database,
  HardDrive,
  RefreshCw,
  Download,
  Activity,
  Clock,
  AlertTriangle
} from 'lucide-vue-next'
import { adminApi } from '../api/admin'
import { useToast } from '../composables/useToast'

const toast = useToast()

const loading = ref(true)
const sampling = ref(false)
const data = ref(null)
let pollTimer = null

async function fetchMetrics() {
  try {
    data.value = await adminApi.getResourceMetrics()
  } catch (err) {
    toast.error('加载资源监控数据失败：' + err.message)
  } finally {
    loading.value = false
  }
}

async function handleSampleNow() {
  if (sampling.value) return
  sampling.value = true
  try {
    data.value = await adminApi.sampleResourceMetrics()
    toast.success('已完成一次实时采样并更新今日审计记录')
  } catch (err) {
    toast.error('触发采样失败：' + err.message)
  } finally {
    sampling.value = false
  }
}

const current = computed(() => data.value?.current || null)
const recentSamples = computed(() => data.value?.recent_samples || [])
const dailyStats = computed(() => data.value?.daily_stats || [])
const todayRow = computed(() => dailyStats.value[0] || null)

const uptimeText = computed(() => {
  const sec = data.value?.uptime_sec || 0
  const days = Math.floor(sec / 86400)
  const hours = Math.floor((sec % 86400) / 3600)
  const mins = Math.floor((sec % 3600) / 60)
  if (days > 0) return `${days} 天 ${hours} 小时 ${mins} 分`
  if (hours > 0) return `${hours} 小时 ${mins} 分`
  return `${mins} 分钟`
})

const scenarioBadgeClass = computed(() => {
  const sc = current.value?.scenario || ''
  if (sc.includes('同步')) return 'bg-sky-500/15 text-sky-300 border-sky-500/30'
  if (sc.includes('播放')) return 'bg-emerald-500/15 text-emerald-300 border-emerald-500/30'
  return 'bg-slate-800/90 text-slate-300 border-slate-700/80'
})

const diskBarClass = computed(() => {
  const pct = current.value?.disk_usage_pct || 0
  if (pct >= 90) return 'from-rose-500 to-amber-500'
  if (pct >= 75) return 'from-amber-500 to-yellow-400'
  return 'from-emerald-500 to-cyan-400'
})

function buildSparklinePoints(values, width = 320, height = 72) {
  if (!values.length) return ''
  const max = Math.max(...values, 1)
  const min = Math.min(...values, 0)
  const span = Math.max(max - min, 0.01)
  const padY = 6
  const innerH = height - padY * 2
  return values
    .map((val, idx) => {
      const x = values.length === 1 ? width / 2 : (idx / (values.length - 1)) * width
      const y = height - padY - ((val - min) / span) * innerH
      return `${x.toFixed(1)},${y.toFixed(1)}`
    })
    .join(' ')
}

const cpuSeries = computed(() => recentSamples.value.map(s => Number(s.app_cpu_pct || 0)))
const memSeries = computed(() => recentSamples.value.map(s => Number(s.app_mem_mb || 0)))

const cpuSparkPoints = computed(() => buildSparklinePoints(cpuSeries.value))
const memSparkPoints = computed(() => buildSparklinePoints(memSeries.value))

const cpuSummary = computed(() => summarizeSeries(cpuSeries.value))
const memSummary = computed(() => summarizeSeries(memSeries.value))

function summarizeSeries(arr) {
  if (!arr.length) return { avg: 0, max: 0 }
  const sum = arr.reduce((a, b) => a + b, 0)
  return {
    avg: (sum / arr.length).toFixed(1),
    max: Math.max(...arr).toFixed(1)
  }
}

function fmtNum(val, digits = 1) {
  const n = Number(val || 0)
  return n.toFixed(digits)
}

function fmtTime(iso) {
  if (!iso) return '-'
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return '-'
  return d.toLocaleTimeString('zh-CN', { hour12: false })
}

function exportAuditCsv() {
  if (!dailyStats.value.length) return
  const headers = [
    '日期',
    '采样次数',
    '服务CPU均值(%)',
    '服务CPU峰值(%)',
    '服务内存均值(MB)',
    '服务内存峰值(MB)',
    'DBCPU均值(%)',
    'DBCPU峰值(%)',
    'DB内存均值(MB)',
    'DB内存峰值(MB)',
    'DB体积峰值(MB)',
    'DB连接均值',
    'DB连接峰值',
    '磁盘已用峰值(GB)',
    '磁盘占用峰值(%)',
    'CPU峰值场景',
    '内存峰值场景'
  ]
  const rows = dailyStats.value.map(r => [
    r.stat_date,
    r.sample_count,
    fmtNum(r.app_cpu_avg, 2),
    fmtNum(r.app_cpu_max, 2),
    fmtNum(r.app_mem_avg_mb, 2),
    fmtNum(r.app_mem_max_mb, 2),
    fmtNum(r.db_cpu_avg, 2),
    fmtNum(r.db_cpu_max, 2),
    fmtNum(r.db_mem_avg_mb, 2),
    fmtNum(r.db_mem_max_mb, 2),
    fmtNum(r.db_size_max_mb, 2),
    fmtNum(r.db_conn_avg, 1),
    r.db_conn_max,
    fmtNum(r.disk_used_max_gb, 2),
    fmtNum(r.disk_usage_max_pct, 2),
    `"${(r.peak_cpu_scenario || '').replace(/"/g, '""')}"`,
    `"${(r.peak_mem_scenario || '').replace(/"/g, '""')}"`
  ])
  const csv = '\uFEFF' + [headers.join(','), ...rows.map(r => r.join(','))].join('\n')
  const blob = new Blob([csv], { type: 'text/csv;charset=utf-8;' })
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = `rx-emby-resource-audit-${new Date().toISOString().slice(0, 10)}.csv`
  a.click()
  URL.revokeObjectURL(url)
}

onMounted(() => {
  fetchMetrics()
  pollTimer = setInterval(fetchMetrics, 30000)
})

onUnmounted(() => {
  if (pollTimer) clearInterval(pollTimer)
})
</script>

<template>
  <section class="space-y-6">
    <!-- 顶部状态条与操作 -->
    <div class="card p-4 rounded-2xl flex flex-wrap items-center justify-between gap-4">
      <div class="flex flex-wrap items-center gap-3 text-xs">
        <span class="inline-flex items-center gap-2 px-3 py-1.5 rounded-xl border font-medium" :class="scenarioBadgeClass">
          <Activity class="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
          <span>当前场景：{{ current?.scenario || '采样初始化…' }}</span>
        </span>
        <span class="inline-flex items-center gap-1.5 text-slate-400 font-mono tabular-nums">
          <Clock class="w-3.5 h-3.5 text-slate-500" aria-hidden="true" />
          <span>连续运行 {{ uptimeText }}</span>
        </span>
        <span class="text-slate-500 hidden sm:inline">·</span>
        <span class="text-slate-400">每 30 秒自动采样并归档每日均值与峰值</span>
      </div>

      <div class="flex items-center gap-2.5">
        <button
          type="button"
          @click="exportAuditCsv"
          :disabled="!dailyStats.length"
          class="cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed px-3 py-1.5 rounded-xl bg-slate-900/90 hover:bg-slate-800 border border-slate-700/80 text-xs font-medium text-slate-200 transition-colors flex items-center gap-1.5"
        >
          <Download class="w-3.5 h-3.5" aria-hidden="true" />
          <span>导出审计 CSV</span>
        </button>

        <button
          type="button"
          @click="handleSampleNow"
          :disabled="sampling"
          :aria-busy="sampling"
          class="cursor-pointer disabled:opacity-60 px-3.5 py-1.5 rounded-xl bg-emerald-600/20 hover:bg-emerald-600/30 border border-emerald-500/35 text-xs font-medium text-emerald-300 transition-colors flex items-center gap-1.5"
        >
          <RefreshCw class="w-3.5 h-3.5" :class="{ 'animate-spin': sampling }" aria-hidden="true" />
          <span>{{ sampling ? '采样中…' : '立即采样' }}</span>
        </button>
      </div>
    </div>

    <div v-if="loading" class="grid grid-cols-1 lg:grid-cols-3 gap-5">
      <div v-for="i in 3" :key="i" class="card p-5 rounded-2xl h-44 animate-pulse bg-slate-900/50"></div>
    </div>

    <template v-else>
      <!-- 三列实时资源卡片：应用服务 / 数据库服务 / 磁盘与缓存 -->
      <div class="grid grid-cols-1 lg:grid-cols-3 gap-5">
        <!-- 1. RX-Emby 应用进程 -->
        <div class="card p-5 rounded-2xl relative overflow-hidden space-y-4">
          <div class="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-emerald-500/80 to-transparent"></div>
          <div class="flex items-center justify-between">
            <div>
              <h2 class="text-sm font-semibold text-white">RX-Emby 服务进程</h2>
              <p class="text-[11px] text-slate-400 font-mono mt-0.5">PID {{ current?.app_pid || '-' }}</p>
            </div>
            <span class="w-9 h-9 rounded-xl bg-emerald-500/15 border border-emerald-500/25 text-emerald-400 flex items-center justify-center">
              <Cpu class="w-4 h-4" aria-hidden="true" />
            </span>
          </div>

          <div class="grid grid-cols-2 gap-3 tabular-nums">
            <div class="rounded-xl bg-slate-950/60 border border-slate-800/80 p-3">
              <div class="text-[11px] text-slate-400">CPU 占用</div>
              <div class="mt-1 text-2xl font-bold text-white font-mono">
                {{ fmtNum(current?.app_cpu_pct, 1) }}<span class="text-xs font-normal text-slate-400 ml-0.5">%</span>
              </div>
            </div>
            <div class="rounded-xl bg-slate-950/60 border border-slate-800/80 p-3">
              <div class="text-[11px] text-slate-400">RSS 内存</div>
              <div class="mt-1 text-2xl font-bold text-emerald-400 font-mono">
                {{ fmtNum(current?.app_mem_mb, 1) }}<span class="text-xs font-normal text-slate-400 ml-1">MB</span>
              </div>
            </div>
          </div>

          <div class="pt-2 border-t border-slate-800/80 flex items-center justify-between text-[11px] text-slate-400 tabular-nums font-mono">
            <span>今日峰值 CPU: <strong class="text-slate-200">{{ fmtNum(todayRow?.app_cpu_max, 1) }}%</strong></span>
            <span>内存: <strong class="text-slate-200">{{ fmtNum(todayRow?.app_mem_max_mb, 1) }} MB</strong></span>
          </div>
        </div>

        <!-- 2. PostgreSQL 数据库服务 -->
        <div class="card p-5 rounded-2xl relative overflow-hidden space-y-4">
          <div class="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-cyan-500/80 to-transparent"></div>
          <div class="flex items-center justify-between">
            <div>
              <h2 class="text-sm font-semibold text-white">PostgreSQL 数据库</h2>
              <p class="text-[11px] text-slate-400 font-mono mt-0.5">
                缓存命中率 {{ fmtNum(current?.db_cache_hit_pct, 1) }}%
              </p>
            </div>
            <span class="w-9 h-9 rounded-xl bg-cyan-500/15 border border-cyan-500/25 text-cyan-400 flex items-center justify-center">
              <Database class="w-4 h-4" aria-hidden="true" />
            </span>
          </div>

          <div class="grid grid-cols-2 gap-3 tabular-nums">
            <div class="rounded-xl bg-slate-950/60 border border-slate-800/80 p-3">
              <div class="text-[11px] text-slate-400">数据库体积</div>
              <div class="mt-1 text-2xl font-bold text-white font-mono">
                {{ fmtNum(current?.db_size_mb, 1) }}<span class="text-xs font-normal text-slate-400 ml-1">MB</span>
              </div>
            </div>
            <div class="rounded-xl bg-slate-950/60 border border-slate-800/80 p-3">
              <div class="text-[11px] text-slate-400">活跃 / 总连接</div>
              <div class="mt-1 text-2xl font-bold text-cyan-400 font-mono">
                {{ current?.db_active_conns || 0 }}<span class="text-sm font-normal text-slate-400"> / {{ current?.db_total_conns || 0 }}</span>
              </div>
            </div>
          </div>

          <div class="pt-2 border-t border-slate-800/80 flex items-center justify-between text-[11px] text-slate-400 tabular-nums font-mono">
            <span v-if="current?.db_host_available">
              DB 进程: <strong class="text-slate-200">{{ fmtNum(current?.db_cpu_pct, 1) }}%</strong> · <strong class="text-slate-200">{{ fmtNum(current?.db_mem_mb, 1) }} MB</strong>
            </span>
            <span v-else>
              连接上限: <strong class="text-slate-200">{{ current?.db_max_conns || 100 }}</strong>（独立容器/远程）
            </span>
            <span>今日峰值连接: <strong class="text-slate-200">{{ todayRow?.db_conn_max || 0 }}</strong></span>
          </div>
        </div>

        <!-- 3. 磁盘与缓存水位 -->
        <div class="card p-5 rounded-2xl relative overflow-hidden space-y-4">
          <div class="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-amber-500/80 to-transparent"></div>
          <div class="flex items-center justify-between">
            <div>
              <h2 class="text-sm font-semibold text-white">磁盘与缓存空间</h2>
              <p class="text-[11px] text-slate-400 font-mono mt-0.5">
                可用 {{ fmtNum(current?.disk_avail_gb, 1) }} GB / 总 {{ fmtNum(current?.disk_total_gb, 1) }} GB
              </p>
            </div>
            <span class="w-9 h-9 rounded-xl bg-amber-500/15 border border-amber-500/25 text-amber-400 flex items-center justify-center">
              <HardDrive class="w-4 h-4" aria-hidden="true" />
            </span>
          </div>

          <div class="space-y-2 tabular-nums">
            <div class="flex items-baseline justify-between">
              <span class="text-2xl font-bold text-white font-mono">
                {{ fmtNum(current?.disk_usage_pct, 1) }}%
              </span>
              <span class="text-xs text-slate-400 font-mono">
                已用 {{ fmtNum(current?.disk_used_gb, 1) }} GB
              </span>
            </div>
            <div class="h-2 w-full bg-slate-800 rounded-full overflow-hidden">
              <div
                class="h-full bg-gradient-to-r rounded-full transition-all duration-500"
                :class="diskBarClass"
                :style="{ width: `${Math.min(100, current?.disk_usage_pct || 0)}%` }"
              ></div>
            </div>
          </div>

          <div class="pt-2 border-t border-slate-800/80 flex items-center justify-between text-[11px] text-slate-400 tabular-nums font-mono">
            <span>图片缓存: <strong class="text-amber-300">{{ fmtNum(current?.image_cache_mb, 1) }} MB</strong></span>
            <span>今日峰值: <strong class="text-slate-200">{{ fmtNum(todayRow?.disk_usage_max_pct, 1) }}%</strong></span>
          </div>
        </div>
      </div>

      <!-- 近期实时采样趋势图（最近 60 次采样） -->
      <div class="grid grid-cols-1 lg:grid-cols-2 gap-5">
        <div class="card p-5 rounded-2xl space-y-3">
          <div class="flex items-center justify-between">
            <div>
              <h3 class="text-sm font-semibold text-white">服务 CPU 实时趋势</h3>
              <p class="text-[11px] text-slate-400 mt-0.5">最近 {{ recentSamples.length }} 次采样（每 30 秒）</p>
            </div>
            <div class="text-xs font-mono tabular-nums text-slate-400">
              均值 <strong class="text-slate-200">{{ cpuSummary.avg }}%</strong> · 峰值 <strong class="text-emerald-400">{{ cpuSummary.max }}%</strong>
            </div>
          </div>
          <div class="rounded-xl bg-slate-950/60 border border-slate-800/80 p-3">
            <svg
              v-if="cpuSeries.length > 1"
              viewBox="0 0 320 72"
              class="w-full h-20 overflow-visible"
              role="img"
              :aria-label="`服务 CPU 趋势图，均值 ${cpuSummary.avg}%，峰值 ${cpuSummary.max}%`"
            >
              <polyline
                fill="none"
                stroke="#10b981"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                :points="cpuSparkPoints"
              />
            </svg>
            <div v-else class="h-20 flex items-center justify-center text-xs text-slate-500">
              采样点累积中（超过 2 个点后自动绘制曲线）
            </div>
          </div>
        </div>

        <div class="card p-5 rounded-2xl space-y-3">
          <div class="flex items-center justify-between">
            <div>
              <h3 class="text-sm font-semibold text-white">服务内存 (RSS) 实时趋势</h3>
              <p class="text-[11px] text-slate-400 mt-0.5">最近 {{ recentSamples.length }} 次采样（单位 MB）</p>
            </div>
            <div class="text-xs font-mono tabular-nums text-slate-400">
              均值 <strong class="text-slate-200">{{ memSummary.avg }} MB</strong> · 峰值 <strong class="text-cyan-400">{{ memSummary.max }} MB</strong>
            </div>
          </div>
          <div class="rounded-xl bg-slate-950/60 border border-slate-800/80 p-3">
            <svg
              v-if="memSeries.length > 1"
              viewBox="0 0 320 72"
              class="w-full h-20 overflow-visible"
              role="img"
              :aria-label="`服务内存趋势图，均值 ${memSummary.avg} MB，峰值 ${memSummary.max} MB`"
            >
              <polyline
                fill="none"
                stroke="#06b6d4"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                :points="memSparkPoints"
              />
            </svg>
            <div v-else class="h-20 flex items-center justify-center text-xs text-slate-500">
              采样点累积中（超过 2 个点后自动绘制曲线）
            </div>
          </div>
        </div>
      </div>

      <!-- 每日资源使用与峰值场景审计表 -->
      <div class="card p-5 rounded-2xl space-y-4">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <div>
            <h3 class="text-base font-bold text-white">每日资源审计记录（近 30 天）</h3>
            <p class="text-xs text-slate-400 mt-0.5">汇总每日日常均值、最大峰值及峰值触发时的业务场景，便于容量评估与问题回溯</p>
          </div>
          <span class="text-xs font-mono text-slate-400">共 {{ dailyStats.length }} 天记录</span>
        </div>

        <div v-if="!dailyStats.length" class="py-10 text-center text-xs text-slate-500">
          暂无每日审计数据，点击右上角「立即采样」即可生成首条记录
        </div>

        <div v-else class="overflow-x-auto">
          <table class="w-full text-left border-collapse text-xs tabular-nums">
            <thead>
              <tr class="border-b border-slate-800 text-slate-400 font-medium">
                <th class="py-2.5 px-3">日期</th>
                <th class="py-2.5 px-3">采样数</th>
                <th class="py-2.5 px-3">服务 CPU (均值 / 峰值)</th>
                <th class="py-2.5 px-3">服务内存 (均值 / 峰值)</th>
                <th class="py-2.5 px-3">DB 体积 / 连接峰值</th>
                <th class="py-2.5 px-3">磁盘峰值</th>
                <th class="py-2.5 px-3">峰值触发场景</th>
              </tr>
            </thead>
            <tbody class="divide-y divide-slate-800/60">
              <tr
                v-for="(row, idx) in dailyStats"
                :key="row.stat_date"
                class="hover:bg-slate-900/50 transition-colors"
              >
                <td class="py-3 px-3 font-mono text-slate-200 whitespace-nowrap">
                  <span>{{ row.stat_date }}</span>
                  <span
                    v-if="idx === 0"
                    class="ml-1.5 px-1.5 py-0.5 text-[10px] rounded bg-emerald-500/15 text-emerald-300 border border-emerald-500/25"
                  >
                    今日
                  </span>
                </td>
                <td class="py-3 px-3 font-mono text-slate-400">{{ row.sample_count }}</td>
                <td class="py-3 px-3 font-mono whitespace-nowrap">
                  <span class="text-slate-300">{{ fmtNum(row.app_cpu_avg, 1) }}%</span>
                  <span class="text-slate-600 mx-1">/</span>
                  <span :class="row.app_cpu_max >= 80 ? 'text-rose-400 font-semibold' : 'text-emerald-400'">
                    {{ fmtNum(row.app_cpu_max, 1) }}%
                  </span>
                </td>
                <td class="py-3 px-3 font-mono whitespace-nowrap">
                  <span class="text-slate-300">{{ fmtNum(row.app_mem_avg_mb, 1) }}</span>
                  <span class="text-slate-600 mx-1">/</span>
                  <span class="text-cyan-400">{{ fmtNum(row.app_mem_max_mb, 1) }} MB</span>
                </td>
                <td class="py-3 px-3 font-mono whitespace-nowrap text-slate-300">
                  <span>{{ fmtNum(row.db_size_max_mb, 1) }} MB</span>
                  <span class="text-slate-600 mx-1">·</span>
                  <span>{{ row.db_conn_max }} 连接</span>
                </td>
                <td class="py-3 px-3 font-mono whitespace-nowrap">
                  <span :class="row.disk_usage_max_pct >= 85 ? 'text-amber-400 font-semibold' : 'text-slate-300'">
                    {{ fmtNum(row.disk_usage_max_pct, 1) }}%
                  </span>
                  <span class="text-slate-500 ml-1">({{ fmtNum(row.disk_used_max_gb, 1) }}G)</span>
                </td>
                <td class="py-3 px-3 text-slate-300">
                  <div class="flex flex-wrap items-center gap-1.5">
                    <span
                      v-if="row.peak_cpu_scenario"
                      class="inline-flex items-center gap-1 px-2 py-0.5 rounded bg-slate-800/90 border border-slate-700/80 text-[11px]"
                      :title="`CPU 峰值发生于 ${fmtTime(row.peak_cpu_at)}`"
                    >
                      <AlertTriangle v-if="row.app_cpu_max >= 80" class="w-3 h-3 text-rose-400" aria-hidden="true" />
                      <span>CPU: {{ row.peak_cpu_scenario }} ({{ fmtTime(row.peak_cpu_at) }})</span>
                    </span>
                    <span
                      v-if="row.peak_mem_scenario && row.peak_mem_scenario !== row.peak_cpu_scenario"
                      class="inline-flex items-center gap-1 px-2 py-0.5 rounded bg-slate-800/90 border border-slate-700/80 text-[11px]"
                      :title="`内存峰值发生于 ${fmtTime(row.peak_mem_at)}`"
                    >
                      <span>内存: {{ row.peak_mem_scenario }} ({{ fmtTime(row.peak_mem_at) }})</span>
                    </span>
                  </div>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </div>
    </template>
  </section>
</template>
