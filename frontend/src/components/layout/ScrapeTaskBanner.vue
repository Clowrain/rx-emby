<template>
  <div
    v-if="visible"
    class="border-b border-slate-800/80 bg-[#0a101d]/95 backdrop-blur-md px-4 sm:px-6 lg:px-8 py-2.5 transition-colors"
    role="region"
    aria-label="后台刮削任务进度"
  >
    <div v-if="showResumeBanner" class="max-w-[1600px] mx-auto flex flex-wrap items-center justify-between gap-3">
      <div class="flex items-center gap-2.5 text-xs text-slate-300">
        <span class="px-2 py-0.5 rounded-full text-[11px] font-semibold bg-slate-800 text-slate-300 border border-slate-700">
          已取消
        </span>
        <span>
          后台刮削已中止，待刮削队列中仍有 <strong class="text-white font-mono">{{ status.pending_remaining }}</strong> 项等待处理。
        </span>
      </div>
      <div class="flex items-center gap-2">
        <button
          type="button"
          :disabled="busyAction !== null"
          @click="handleResume"
          class="cursor-pointer disabled:opacity-50 px-3 py-1 rounded-lg bg-emerald-600 hover:bg-emerald-500 text-white text-[11px] font-medium flex items-center gap-1 transition-colors"
        >
          <Play class="w-3 h-3" aria-hidden="true" />
          <span>重新开始刮削</span>
        </button>
        <button
          type="button"
          @click="dismissedCancelledBanner = true"
          class="cursor-pointer px-2.5 py-1 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-300 text-[11px]"
        >
          暂不提示
        </button>
      </div>
    </div>

    <div v-else class="max-w-[1600px] mx-auto flex flex-col gap-2">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <div class="flex items-center gap-2.5 min-w-0">
          <span
            class="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full text-[11px] font-semibold border"
            :class="
              isPaused
                ? 'bg-amber-500/15 text-amber-300 border-amber-500/35'
                : 'bg-emerald-500/15 text-emerald-300 border-emerald-500/35'
            "
          >
            <span
              class="w-1.5 h-1.5 rounded-full"
              :class="isPaused ? 'bg-amber-400' : 'bg-emerald-400 animate-pulse'"
            ></span>
            <span>{{ isPaused ? '已暂停' : '刮削中' }}</span>
          </span>

          <span class="text-xs font-semibold text-slate-100 truncate">
            {{ taskTitle }}
          </span>

          <span
            v-if="status.current_item"
            class="text-[11px] text-slate-400 truncate max-w-[320px] hidden sm:inline"
            :title="status.current_item"
          >
            正在处理：{{ status.current_item }}
          </span>
        </div>

        <div class="flex items-center gap-3 tabular-nums">
          <div class="text-[11px] text-slate-300 flex items-center gap-2 font-mono">
            <template v-if="status.batch_total > 0">
              <span>
                <strong class="text-white">{{ status.batch_done }}</strong> / {{ status.batch_total }}
                <span class="text-emerald-300 ml-1">({{ displayPercent }}%)</span>
              </span>
              <span class="text-slate-600">|</span>
              <span class="text-emerald-400">成功 {{ status.batch_succeeded }}</span>
              <span class="text-amber-400">失败/存疑 {{ status.batch_failed }}</span>
            </template>
            <template v-else>
              <span>队列待处理：{{ status.pending_remaining || 0 }} 项</span>
            </template>
          </div>

          <div class="flex items-center gap-1.5">
            <button
              v-if="!isPaused"
              type="button"
              :disabled="busyAction !== null"
              @click="handlePause"
              class="cursor-pointer disabled:opacity-50 px-2.5 py-1 rounded-lg bg-amber-500/15 hover:bg-amber-500/25 border border-amber-500/35 text-amber-200 text-[11px] font-medium flex items-center gap-1 transition-colors"
            >
              <Pause class="w-3 h-3" aria-hidden="true" />
              <span>暂停</span>
            </button>
            <button
              v-else
              type="button"
              :disabled="busyAction !== null"
              @click="handleResume"
              class="cursor-pointer disabled:opacity-50 px-2.5 py-1 rounded-lg bg-emerald-500/15 hover:bg-emerald-500/25 border border-emerald-500/35 text-emerald-200 text-[11px] font-medium flex items-center gap-1 transition-colors"
            >
              <Play class="w-3 h-3" aria-hidden="true" />
              <span>继续</span>
            </button>
            <button
              type="button"
              :disabled="busyAction !== null"
              @click="handleCancel"
              class="cursor-pointer disabled:opacity-50 px-2.5 py-1 rounded-lg bg-rose-500/15 hover:bg-rose-500/25 border border-rose-500/35 text-rose-200 text-[11px] font-medium flex items-center gap-1 transition-colors"
            >
              <X class="w-3 h-3" aria-hidden="true" />
              <span>取消</span>
            </button>
            <button
              type="button"
              @click="$emit('open-tasks')"
              class="cursor-pointer px-2.5 py-1 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-200 text-[11px] font-medium transition-colors"
            >
              任务中心
            </button>
          </div>
        </div>
      </div>

      <div class="w-full h-1.5 rounded-full bg-slate-800/90 overflow-hidden">
        <div
          v-if="status.batch_total > 0"
          class="h-full rounded-full transition-all duration-300"
          :class="isPaused ? 'bg-amber-400/80' : 'bg-gradient-to-r from-emerald-500 to-cyan-400'"
          :style="{ width: `${displayPercent}%` }"
        ></div>
        <div
          v-else
          class="h-full w-1/3 rounded-full"
          :class="isPaused ? 'bg-amber-400/60' : 'bg-emerald-500/80 animate-pulse'"
        ></div>
      </div>
    </div>
  </div>
</template>

<script setup>
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { Pause, Play, X } from 'lucide-vue-next'
import { adminApi } from '../../api/admin'
import { useToast, useConfirm } from '../../composables/useToast'

const emit = defineEmits(['completed', 'open-tasks'])

const toast = useToast()
const confirmModal = useConfirm()

const status = ref({
  status: 'idle',
  running: false,
  paused: false,
  task_name: null,
  current_item: null,
  batch_total: 0,
  batch_done: 0,
  batch_succeeded: 0,
  batch_failed: 0,
  pending_remaining: 0,
  percent: null
})
const busyAction = ref(null)
const dismissedCancelledBanner = ref(false)
let timer = null
let prevStatus = 'idle'
let prevBatchTotal = 0

const showResumeBanner = computed(
  () =>
    status.value.status === 'idle' &&
    Boolean(status.value.can_resume_auto) &&
    !dismissedCancelledBanner.value
)
const visible = computed(
  () =>
    status.value.status === 'running' ||
    status.value.status === 'paused' ||
    showResumeBanner.value
)
const isPaused = computed(() => status.value.status === 'paused')
const taskTitle = computed(() => status.value.task_name || '后台 TMDB 刮削队列')
const displayPercent = computed(() => {
  if (typeof status.value.percent === 'number') {
    return Math.min(100, Math.max(0, status.value.percent))
  }
  if (status.value.batch_total > 0) {
    return Math.round((status.value.batch_done / status.value.batch_total) * 1000) / 10
  }
  return 0
})

async function pollStatus() {
  try {
    const next = await adminApi.getScrapeStatus()
    if (prevStatus === 'running' && next.status === 'idle' && prevBatchTotal > 0) {
      toast.success(
        `刮削任务已完成：共处理 ${next.batch_done || prevBatchTotal} 项（成功 ${next.batch_succeeded || 0}，失败/存疑 ${next.batch_failed || 0}）`
      )
      emit('completed')
      window.dispatchEvent(new CustomEvent('rx-scrape-task-completed'))
    }
    if (next.status === 'running' || next.status === 'paused') {
      dismissedCancelledBanner.value = false
    }
    prevStatus = next.status
    prevBatchTotal = next.batch_total || 0
    status.value = next
  } catch {
    // 静默忽略轮询偶发超时
  } finally {
    scheduleNextPoll()
  }
}

function scheduleNextPoll() {
  if (timer) clearTimeout(timer)
  const delay = status.value.status === 'running' ? 1800 : status.value.status === 'paused' ? 4000 : 10000
  timer = setTimeout(pollStatus, delay)
}

async function handlePause() {
  if (busyAction.value) return
  busyAction.value = 'pause'
  try {
    const res = await adminApi.pauseScrapeTask()
    status.value = res
    prevStatus = res.status
    toast.info(res.message || '已暂停刮削任务')
  } catch (err) {
    toast.error('暂停失败：' + err.message)
  } finally {
    busyAction.value = null
  }
}

async function handleResume() {
  if (busyAction.value) return
  busyAction.value = 'resume'
  try {
    const res = await adminApi.resumeScrapeTask()
    status.value = res
    prevStatus = res.status
    toast.success(res.message || '已恢复刮削任务')
    scheduleNextPoll()
  } catch (err) {
    toast.error('恢复失败：' + err.message)
  } finally {
    busyAction.value = null
  }
}

async function handleCancel() {
  if (busyAction.value) return
  const ok = await confirmModal({
    title: '取消刮削任务',
    message: '确定要取消当前刮削任务吗？已完成的条目会保留，未处理的条目将停止拉取。',
    confirmText: '确认取消',
    danger: true
  })
  if (!ok) return
  busyAction.value = 'cancel'
  try {
    const res = await adminApi.cancelScrapeTask()
    status.value = res
    prevStatus = res.status
    prevBatchTotal = 0
    toast.info(res.message || '已取消当前刮削队列')
    emit('completed')
  } catch (err) {
    toast.error('取消失败：' + err.message)
  } finally {
    busyAction.value = null
  }
}

function handleExternalStart() {
  pollStatus()
}

onMounted(() => {
  window.addEventListener('rx-scrape-task-started', handleExternalStart)
  pollStatus()
})

onUnmounted(() => {
  window.removeEventListener('rx-scrape-task-started', handleExternalStart)
  if (timer) clearTimeout(timer)
})
</script>
