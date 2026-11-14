<template>
  <section class="space-y-6">
    <div class="grid grid-cols-1 lg:grid-cols-2 gap-6">
      <!-- 增量同步引擎卡片 -->
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

      <!-- 本地图片 LRU 磁盘缓存水位卡片 -->
      <div class="card p-6 rounded-2xl flex flex-col justify-between gap-4">
        <div class="space-y-2.5">
          <div class="flex items-center justify-between gap-3">
            <h2 class="text-base font-bold text-white flex items-center gap-2 min-w-0">
              <HardDrive class="w-4 h-4 text-amber-400 shrink-0" aria-hidden="true" />
              <span class="truncate">本地图片缓存管理</span>
            </h2>
            <button
              type="button"
              @click="handleTriggerEvict"
              class="cursor-pointer shrink-0 whitespace-nowrap bg-amber-600/20 hover:bg-amber-600/35 border border-amber-500/40 text-amber-300 px-3.5 py-1.5 rounded-xl text-xs font-semibold transition-colors flex items-center gap-1.5"
            >
              <Eraser class="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
              <span>立即清理过期缓存</span>
            </button>
          </div>
          <p class="text-xs text-slate-400 leading-relaxed">
            自动缓存客户端请求的海报与剧照，达高水位时自动清理最久未访问的图片。
          </p>
        </div>

        <div v-if="!cacheStats" class="text-xs text-slate-500 py-4">正在读取磁盘缓存水位…</div>
        <div v-else class="space-y-2.5 pt-1 tabular-nums">
          <div class="flex flex-wrap items-baseline justify-between gap-2 text-xs">
            <span class="text-slate-400 whitespace-nowrap">
              已缓存 <strong class="text-white font-mono">{{ Number(cacheStats.file_count).toLocaleString() }}</strong> 张海报与剧照
            </span>
            <span class="font-mono text-emerald-400 font-semibold whitespace-nowrap">
              {{ cacheStats.used_mb }} MB / {{ cacheStats.max_mb }} MB ({{ cacheStats.usage_percent }})
            </span>
          </div>

          <div class="h-2.5 w-full bg-slate-950 rounded-full overflow-hidden border border-slate-800">
            <div
              class="h-full bg-gradient-to-r from-emerald-500 to-amber-500 rounded-full transition-all duration-500"
              :style="{ width: cacheStats.usage_percent || '1%' }"
            ></div>
          </div>

          <div class="flex items-center justify-between gap-2 text-[11px] text-slate-500 font-mono whitespace-nowrap">
            <span>高水位触发线: {{ (cacheStats.high_watermark * 100).toFixed(0) }}%</span>
            <span>低水位安全线: {{ (cacheStats.low_watermark * 100).toFixed(0) }}%</span>
          </div>
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
  </section>
</template>

<script setup>
import { ref, onMounted } from 'vue'
import { Database, Zap, RefreshCw, HardDrive, Eraser, History } from 'lucide-vue-next'
import { adminApi } from '../api/admin'
import { useToast } from '../composables/useToast'

const toast = useToast()
const cacheStats = ref(null)
const logs = ref([])
const syncing = ref(false)

async function loadAll() {
  try {
    const [cache, logsData] = await Promise.all([
      adminApi.getCacheStats(),
      adminApi.listSyncLogs()
    ])
    cacheStats.value = cache
    logs.value = logsData.logs || []
  } catch (err) {
    toast.error('加载同步与缓存状态失败：' + err.message)
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

async function handleTriggerEvict() {
  try {
    const data = await adminApi.triggerCacheEvict()
    toast.success(`缓存清理完成，已释放磁盘空间 ${data.freed_mb} MB`)
    await loadAll()
  } catch (err) {
    toast.error('清理缓存失败：' + err.message)
  }
}

onMounted(loadAll)
</script>
