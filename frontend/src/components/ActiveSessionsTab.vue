<template>
  <section class="space-y-6">
    <div class="card p-6 rounded-2xl">
      <div class="flex flex-wrap items-center justify-between gap-3 mb-4">
        <div>
          <h2 class="text-base font-bold text-white flex items-center gap-2">
            <Activity class="w-4 h-4 text-emerald-400" aria-hidden="true" />
            <span>实时播放会话</span>
          </h2>
          <p class="text-xs text-slate-400 mt-0.5">
            展示当前正在播放的客户端会话。单集播放至 85% 时自动预热下一集直链，达 90% 时自动标记为已看。
          </p>
        </div>
        <button
          type="button"
          @click="loadSessions"
          :disabled="loading"
          class="cursor-pointer bg-slate-800 hover:bg-slate-700 disabled:opacity-50 px-3.5 py-1.5 rounded-xl text-xs font-medium transition-colors flex items-center gap-1.5"
        >
          <RefreshCw class="w-3.5 h-3.5 text-slate-300" :class="{ 'animate-spin': loading }" aria-hidden="true" />
          <span>刷新状态</span>
        </button>
      </div>

      <div
        v-if="activeSessions.length === 0"
        class="bg-slate-950/60 border border-slate-800/80 rounded-xl p-10 text-center space-y-1.5"
      >
        <p class="text-xs font-medium text-slate-400">当前没有正在播放的会话</p>
        <p class="text-[11px] text-slate-500">当 VidHub、Infuse 或其他 Emby 客户端开始播放时，将在此实时显示进度与直链预热状态。</p>
      </div>

      <div v-else class="grid grid-cols-1 md:grid-cols-2 gap-4 text-sm">
        <div
          v-for="s in activeSessions"
          :key="s.session_id"
          class="bg-slate-950/80 border border-emerald-500/30 rounded-xl p-4 space-y-2.5 tabular-nums"
        >
          <div class="flex items-center justify-between gap-2">
            <div class="flex items-center gap-2 min-w-0">
              <span class="w-2 h-2 rounded-full bg-emerald-400 animate-ping shrink-0"></span>
              <span class="font-bold text-white text-sm truncate">{{ s.playing_title }}</span>
            </div>
            <span
              class="px-2 py-0.5 rounded-md text-[10px] font-medium shrink-0 flex items-center gap-1"
              :class="
                s.next_ep_prewarmed
                  ? 'bg-indigo-950 text-indigo-300 border border-indigo-500/40'
                  : 'bg-emerald-950 text-emerald-300 border border-emerald-500/40'
              "
            >
              <FastForward v-if="s.next_ep_prewarmed" class="w-3 h-3" aria-hidden="true" />
              <Play v-else class="w-3 h-3 fill-current" aria-hidden="true" />
              <span>{{ s.next_ep_prewarmed ? '下一集已预热' : '直链播放中' }}</span>
            </span>
          </div>
          <div class="text-xs text-slate-300 flex items-center justify-between font-mono">
            <span>已播放进度: {{ Math.floor(s.position_sec / 60) }}分{{ s.position_sec % 60 }}秒</span>
            <span class="text-slate-500">心跳: {{ s.idle_sec }}s 前</span>
          </div>
          <div class="text-[11px] text-slate-400 truncate">
            客户端设备: {{ s.client_name }} · 用户 ID: <span class="font-mono">{{ s.user_id }}</span>
          </div>
        </div>
      </div>
    </div>
  </section>
</template>

<script setup>
import { ref, onMounted } from 'vue'
import { Activity, RefreshCw, FastForward, Play } from 'lucide-vue-next'
import { adminApi } from '../api/admin'
import { useToast } from '../composables/useToast'

const toast = useToast()
const loading = ref(false)
const activeSessions = ref([])

async function loadSessions() {
  loading.value = true
  try {
    const data = await adminApi.listUsersAndSessions()
    activeSessions.value = data.active_sessions || []
  } catch (err) {
    toast.error('加载实时会话失败：' + err.message)
  } finally {
    loading.value = false
  }
}

onMounted(loadSessions)
</script>
