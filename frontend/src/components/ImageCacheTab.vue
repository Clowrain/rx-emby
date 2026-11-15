<template>
  <section class="space-y-6">
    <!-- 实时磁盘缓存水位卡片 -->
    <div class="card p-6 rounded-2xl space-y-4">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2 class="text-base font-bold text-white flex items-center gap-2">
            <HardDrive class="w-4 h-4 text-amber-400 shrink-0" aria-hidden="true" />
            <span>本地图片缓存水位</span>
            <span
              v-if="cacheStats && cacheStats.disk_cache_enabled === false"
              class="px-2 py-0.5 text-[11px] font-medium rounded-md bg-cyan-950 text-cyan-300 border border-cyan-500/30"
            >
              直通模式（不落盘）
            </span>
          </h2>
          <p class="text-xs text-slate-400 mt-1">
            自动缓存客户端请求的海报与剧照，达到高水位线时自动清理最久未访问的图片。
          </p>
        </div>

        <div class="flex items-center gap-2">
          <button
            type="button"
            @click="loadCacheStats"
            class="cursor-pointer bg-slate-800 hover:bg-slate-700 px-3.5 py-2 rounded-xl text-xs font-medium text-slate-200 transition-colors flex items-center gap-1.5"
          >
            <RefreshCw class="w-3.5 h-3.5 text-slate-300" aria-hidden="true" />
            <span>刷新水位</span>
          </button>
          <button
            type="button"
            @click="handleTriggerEvict"
            :disabled="evicting"
            class="cursor-pointer bg-amber-600/20 hover:bg-amber-600/35 disabled:opacity-50 border border-amber-500/40 text-amber-300 px-4 py-2 rounded-xl text-xs font-semibold transition-colors flex items-center gap-1.5"
          >
            <Eraser class="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
            <span>{{ evicting ? '正在清理…' : '立即清理过期缓存' }}</span>
          </button>
        </div>
      </div>

      <div v-if="!cacheStats" class="text-xs text-slate-500 py-4">正在读取磁盘缓存水位…</div>
      <div v-else class="space-y-2.5 pt-1 tabular-nums">
        <div class="flex flex-wrap items-baseline justify-between gap-2 text-xs">
          <span class="text-slate-400">
            已缓存 <strong class="text-white font-mono">{{ Number(cacheStats.file_count).toLocaleString() }}</strong> 张海报与剧照
          </span>
          <span class="font-mono text-emerald-400 font-semibold">
            {{ cacheStats.used_mb }} MB / {{ cacheStats.max_mb }} MB ({{ cacheStats.usage_percent }})
          </span>
        </div>

        <div class="h-2.5 w-full bg-slate-950 rounded-full overflow-hidden border border-slate-800">
          <div
            class="h-full bg-gradient-to-r from-emerald-500 to-amber-500 rounded-full transition-all duration-500"
            :style="{ width: cacheStats.usage_percent || '1%' }"
          ></div>
        </div>

        <div class="flex items-center justify-between gap-2 text-[11px] text-slate-500 font-mono">
          <span>高水位触发线: {{ (cacheStats.high_watermark * 100).toFixed(0) }}%</span>
          <span>低水位安全线: {{ (cacheStats.low_watermark * 100).toFixed(0) }}%</span>
        </div>
      </div>
    </div>

    <!-- 就近配置缓存目录与配额上限 -->
    <ConfigGroupPanel
      prefix="image_cache."
      title="缓存配额与淘汰参数"
      description="配置本地磁盘图片缓存目录、最大空间配额上限（MB）及 LRU 高低水位淘汰阈值，保存后立即生效。"
      :icon="SlidersHorizontal"
      @saved="handleConfigSaved"
    />
  </section>
</template>

<script setup>
import { ref, onMounted } from 'vue'
import { HardDrive, Eraser, RefreshCw, SlidersHorizontal } from 'lucide-vue-next'
import { adminApi } from '../api/admin'
import { useToast } from '../composables/useToast'
import ConfigGroupPanel from './ConfigGroupPanel.vue'

const emit = defineEmits(['saved'])
const toast = useToast()
const cacheStats = ref(null)
const evicting = ref(false)

async function loadCacheStats() {
  try {
    cacheStats.value = await adminApi.getCacheStats()
  } catch (err) {
    toast.error('加载缓存水位失败：' + err.message)
  }
}

async function handleTriggerEvict() {
  if (evicting.value) return
  evicting.value = true
  try {
    const data = await adminApi.triggerCacheEvict()
    toast.success(`缓存清理完成，已释放磁盘空间 ${data.freed_mb} MB`)
    await loadCacheStats()
    emit('saved')
  } catch (err) {
    toast.error('清理缓存失败：' + err.message)
  } finally {
    evicting.value = false
  }
}

async function handleConfigSaved() {
  await loadCacheStats()
  emit('saved')
}

onMounted(loadCacheStats)
</script>
