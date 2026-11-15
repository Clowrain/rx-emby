<script setup>
import { ref, onUnmounted } from 'vue'
import { GitCommitHorizontal, Copy, Check, ExternalLink } from 'lucide-vue-next'
import { getBuildInfo } from '../../utils/buildInfo'

defineProps({
  variant: {
    type: String,
    default: 'sidebar',
    validator: value => ['sidebar', 'login', 'footer'].includes(value)
  }
})

const buildInfo = getBuildInfo()
const copied = ref(false)
let resetTimer = null

async function copyBuildInfo() {
  const text = buildInfo.diagnosticText
  let ok = false

  if (typeof navigator !== 'undefined' && navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text)
      ok = true
    } catch {
      ok = false
    }
  }

  if (!ok && typeof document !== 'undefined') {
    const textarea = document.createElement('textarea')
    textarea.value = text
    textarea.setAttribute('readonly', '')
    textarea.style.position = 'fixed'
    textarea.style.opacity = '0'
    document.body.appendChild(textarea)
    textarea.select()
    try {
      ok = document.execCommand('copy')
    } catch {
      ok = false
    } finally {
      document.body.removeChild(textarea)
    }
  }

  if (ok) {
    copied.value = true
    if (resetTimer) clearTimeout(resetTimer)
    resetTimer = setTimeout(() => {
      copied.value = false
    }, 2000)
  }
}

onUnmounted(() => {
  if (resetTimer) clearTimeout(resetTimer)
})
</script>

<template>
  <!-- 侧边栏底部构建版本信息 -->
  <button
    v-if="variant === 'sidebar'"
    type="button"
    @click="copyBuildInfo"
    :aria-label="`复制构建版本信息：${buildInfo.diagnosticText}`"
    class="w-full text-left pt-2.5 mt-1 border-t border-slate-800/70 group cursor-pointer rounded-lg px-1.5 py-1.5 -mx-1.5 hover:bg-slate-900/70 transition-colors"
  >
    <div class="flex items-center justify-between gap-2 text-[11px] font-mono tabular-nums">
      <span class="flex items-center gap-1.5 min-w-0 text-slate-300">
        <GitCommitHorizontal class="w-3.5 h-3.5 text-emerald-400/80 shrink-0" aria-hidden="true" />
        <span class="font-semibold text-slate-200 shrink-0">{{ buildInfo.version }}</span>
        <span class="text-slate-600" aria-hidden="true">·</span>
        <span
          class="truncate"
          :class="buildInfo.gitDirty ? 'text-amber-300' : 'text-slate-400'"
        >
          {{ buildInfo.commitLabel }}
        </span>
      </span>

      <span
        class="shrink-0 inline-flex items-center gap-1 text-[10px] px-1.5 py-0.5 rounded border transition-colors"
        :class="
          copied
            ? 'bg-emerald-500/15 text-emerald-300 border-emerald-500/35'
            : 'bg-slate-900/80 text-slate-400 border-slate-800 group-hover:text-slate-200 group-hover:border-slate-700'
        "
      >
        <Check v-if="copied" class="w-3 h-3" aria-hidden="true" />
        <Copy v-else class="w-3 h-3" aria-hidden="true" />
        <span role="status" aria-live="polite">{{ copied ? '已复制' : '复制' }}</span>
      </span>
    </div>

    <div
      v-if="buildInfo.buildTime || (buildInfo.gitBranch && buildInfo.gitBranch !== 'HEAD')"
      class="mt-1 pl-5 flex items-center justify-between gap-2 text-[10px] font-mono tabular-nums text-slate-400/90 truncate"
    >
      <span v-if="buildInfo.gitBranch && buildInfo.gitBranch !== 'HEAD'" class="truncate">
        {{ buildInfo.gitBranch }}
      </span>
      <span v-if="buildInfo.buildTime" class="shrink-0 ml-auto">
        {{ buildInfo.buildTime }}
      </span>
    </div>
  </button>

  <!-- 页面底部 Footer 构建版本信息 -->
  <div v-else-if="variant === 'footer'" class="flex flex-wrap items-center gap-x-2.5 gap-y-1.5 text-xs font-mono tabular-nums text-slate-400">
    <span class="inline-flex items-center gap-1.5 text-slate-200 font-semibold">
      <GitCommitHorizontal class="w-3.5 h-3.5 text-emerald-400 shrink-0" aria-hidden="true" />
      <span>{{ buildInfo.version }}</span>
    </span>

    <span class="text-slate-600" aria-hidden="true">·</span>

    <a
      v-if="buildInfo.commitUrl"
      :href="buildInfo.commitUrl"
      target="_blank"
      rel="noopener noreferrer"
      :aria-label="`在 GitHub 查看提交记录 ${buildInfo.commitLabel}（新窗口打开）`"
      :title="buildInfo.gitDirty ? '查看对应 GitHub 提交记录（当前工作区含未提交改动）' : '在 GitHub 查看此提交记录'"
      class="inline-flex items-center gap-1 px-2 py-0.5 rounded-md border border-slate-800/90 bg-slate-900/70 hover:bg-slate-800 hover:border-emerald-500/40 transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-emerald-500/60"
      :class="buildInfo.gitDirty ? 'text-amber-300 hover:text-amber-200' : 'text-slate-300 hover:text-emerald-300'"
    >
      <span>{{ buildInfo.commitLabel }}</span>
      <span v-if="buildInfo.gitBranch && buildInfo.gitBranch !== 'HEAD'" class="text-slate-400">
        ({{ buildInfo.gitBranch }})
      </span>
      <ExternalLink class="w-3 h-3 shrink-0 opacity-80" aria-hidden="true" />
    </a>
    <span
      v-else
      class="inline-flex items-center gap-1"
      :class="buildInfo.gitDirty ? 'text-amber-300' : 'text-slate-300'"
    >
      <span>{{ buildInfo.commitLabel }}</span>
      <span v-if="buildInfo.gitBranch && buildInfo.gitBranch !== 'HEAD'" class="text-slate-400">
        ({{ buildInfo.gitBranch }})
      </span>
    </span>

    <template v-if="buildInfo.buildTime">
      <span class="text-slate-600" aria-hidden="true">·</span>
      <span class="text-slate-400">
        <span class="text-slate-500 mr-1">构建于</span>{{ buildInfo.buildTime }}
      </span>
    </template>
  </div>

  <!-- 登录页底部构建版本信息 -->
  <div v-else class="mt-4 flex justify-center">
    <button
      type="button"
      @click="copyBuildInfo"
      :aria-label="`复制构建版本信息：${buildInfo.diagnosticText}`"
      class="cursor-pointer inline-flex items-center gap-2 px-3 py-1.5 rounded-full bg-slate-900/70 hover:bg-slate-900 border border-slate-800/80 hover:border-slate-700 text-[11px] font-mono tabular-nums text-slate-400 hover:text-slate-200 transition-colors"
    >
      <GitCommitHorizontal class="w-3.5 h-3.5 text-emerald-400/80 shrink-0" aria-hidden="true" />
      <span class="text-slate-300 font-medium">{{ buildInfo.version }}</span>
      <span class="text-slate-600" aria-hidden="true">·</span>
      <span :class="buildInfo.gitDirty ? 'text-amber-300' : 'text-slate-400'">
        {{ buildInfo.commitLabel }}
      </span>
      <template v-if="buildInfo.buildTime">
        <span class="text-slate-600" aria-hidden="true">·</span>
        <span>{{ buildInfo.buildTime }}</span>
      </template>
      <span
        class="inline-flex items-center gap-1 pl-1.5 ml-0.5 border-l border-slate-800"
        :class="copied ? 'text-emerald-400' : 'text-slate-400'"
      >
        <Check v-if="copied" class="w-3 h-3" aria-hidden="true" />
        <Copy v-else class="w-3 h-3" aria-hidden="true" />
        <span role="status" aria-live="polite" class="sr-only">{{ copied ? '已复制构建信息' : '' }}</span>
      </span>
    </button>
  </div>
</template>
