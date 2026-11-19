<template>
  <Teleport to="body">
    <div
      v-if="folder"
      class="fixed inset-0 z-50 flex justify-end bg-slate-950/70 backdrop-blur-xs"
      @click.self="emit('close')"
    >
      <aside
        class="w-full max-w-xl bg-slate-900 sm:border-l border-slate-800 h-full flex flex-col shadow-2xl overscroll-contain"
        role="dialog"
        aria-modal="true"
        aria-label="目录刮削与校验排查"
      >
        <div class="px-4 sm:px-5 pt-[max(1rem,env(safe-area-inset-top))] pb-3.5 sm:py-4 border-b border-slate-800 flex items-start justify-between gap-3">
          <div class="min-w-0">
            <div class="flex flex-wrap items-center gap-2">
              <h3 class="text-base font-bold text-white truncate">
                {{ folder.parsed_title }}
              </h3>
              <span v-if="folder.parsed_year" class="text-xs font-mono text-slate-400">
                ({{ folder.parsed_year }})
              </span>
              <span
                class="px-2 py-0.5 rounded-md text-[11px] font-medium"
                :class="
                  folder.scrape_state === 'conflict'
                    ? 'bg-amber-950 text-amber-300 border border-amber-500/40'
                    : folder.scrape_state === 'failed'
                      ? 'bg-rose-950 text-rose-300 border border-rose-500/40'
                      : 'bg-slate-800 text-slate-300'
                "
              >
                {{ stateLabel(folder.scrape_state) }}
              </span>
            </div>
            <p class="text-xs font-mono text-slate-400 break-all mt-1">{{ folder.folder_path }}</p>
          </div>
          <button
            type="button"
            @click="emit('close')"
            class="cursor-pointer w-10 h-10 sm:w-8 sm:h-8 rounded-xl bg-slate-800/80 hover:bg-slate-700 text-slate-300 flex items-center justify-center shrink-0"
            aria-label="关闭排查抽屉"
          >
            <X class="w-4 h-4" />
          </button>
        </div>

        <div class="flex-1 overflow-y-auto overscroll-contain p-4 sm:p-5 pb-[max(1.25rem,env(safe-area-inset-bottom))] space-y-5">
          <!-- 诊断原因 -->
          <div
            role="alert"
            class="rounded-xl px-4 py-3 border text-xs space-y-1.5"
            :class="
              folder.scrape_state === 'failed'
                ? 'bg-rose-950/50 border-rose-500/40 text-rose-200'
                : 'bg-amber-950/50 border-amber-500/40 text-amber-200'
            "
          >
            <div class="font-semibold flex items-center justify-between gap-2">
              <span>{{ folder.scrape_state === 'failed' ? '刮削失败原因' : '校验存疑原因' }}</span>
              <span v-if="folder.retry_count > 0" class="font-mono text-[11px] opacity-80">
                已重试 {{ folder.retry_count }} 次
              </span>
            </div>
            <p class="leading-relaxed break-words">
              {{
                folder.conflict_reason ||
                  '历史未匹配记录（当时未保存具体报错），点击下方「立即重新刮削」即可采集完整失败原因。'
              }}
            </p>
            <p
              v-if="hasS0MultiFiles"
              class="text-[11px] text-amber-300/90 pt-1 border-t border-amber-500/20"
            >
              排查提示：检测到当前目录包含多个 S0 视频文件，若它们属于同一部电影的不同版本/压制组（而非分集），可直接点击「按电影绑定提示 TMDB」。
            </p>
          </div>

          <!-- 解析元数据摘要 -->
          <div class="grid grid-cols-2 sm:grid-cols-4 gap-2.5 text-xs">
            <div class="bg-slate-950/70 border border-slate-800 rounded-xl p-2.5">
              <div class="text-[11px] text-slate-400">推断类型</div>
              <div class="font-medium text-white mt-0.5">
                {{ folder.media_type === 'movie' ? '电影 (movie)' : '剧集 (tv)' }}
              </div>
            </div>
            <div class="bg-slate-950/70 border border-slate-800 rounded-xl p-2.5">
              <div class="text-[11px] text-slate-400">所属分类</div>
              <div class="font-medium text-white mt-0.5">{{ folder.category }}</div>
            </div>
            <div class="bg-slate-950/70 border border-slate-800 rounded-xl p-2.5">
              <div class="text-[11px] text-slate-400">提示 TMDB</div>
              <div class="mt-0.5">
                <a
                  v-if="folder.hint_tmdb_id"
                  :href="tmdbUrl(folder)"
                  target="_blank"
                  rel="noopener noreferrer"
                  class="font-mono text-sky-300 hover:underline inline-flex items-center gap-1"
                >
                  <span>#{{ folder.hint_tmdb_id }}</span>
                  <ExternalLink class="w-3 h-3" aria-hidden="true" />
                </a>
                <span v-else class="text-slate-500">无</span>
              </div>
            </div>
            <div class="bg-slate-950/70 border border-slate-800 rounded-xl p-2.5">
              <div class="text-[11px] text-slate-400">视频文件数</div>
              <div class="font-mono font-medium text-emerald-300 mt-0.5">
                {{ folder.file_count }} 个
              </div>
            </div>
          </div>

          <!-- 目录内视频文件明细 -->
          <div class="space-y-2">
            <div class="flex items-center justify-between">
              <h4 class="text-xs font-semibold text-slate-200">
                目录内文件解析明细（最多展示 500 条）
              </h4>
              <span class="text-[11px] text-slate-400 font-mono">
                {{ filesLoading ? '加载中…' : `${files.length} 个文件` }}
              </span>
            </div>

            <div v-if="filesLoading" class="space-y-2">
              <div v-for="n in 3" :key="n" class="h-12 rounded-xl bg-slate-800/60 animate-pulse"></div>
            </div>

            <div
              v-else-if="files.length === 0"
              class="rounded-xl border border-slate-800 bg-slate-950/60 p-6 text-center text-xs text-slate-400"
            >
              该目录下暂无视频文件记录
            </div>

            <div v-else class="space-y-2">
              <div
                v-for="file in files"
                :key="file.id"
                class="rounded-xl border border-slate-800 bg-slate-950/70 p-3 space-y-1.5"
              >
                <div class="flex items-start justify-between gap-2">
                  <span class="text-xs font-medium text-slate-100 break-all">{{ file.file_name }}</span>
                  <span class="px-1.5 py-0.5 rounded bg-slate-800 text-[10px] font-mono text-sky-300 shrink-0">
                    S{{ String(file.season_number).padStart(2, '0') }}E{{ String(file.episode_number).padStart(2, '0') }}
                  </span>
                </div>
                <div class="text-[11px] font-mono text-slate-400 bg-slate-900/80 border border-slate-800/80 rounded-lg px-2.5 py-1.5 break-all select-all">
                  {{ file.alist_path }}
                </div>
                <div class="flex flex-wrap items-center gap-2 text-[11px] text-slate-400">
                  <span class="px-1.5 py-0.5 rounded bg-slate-900 border border-slate-800 text-slate-300">
                    {{ file.version_tag }}
                  </span>
                  <span class="font-mono uppercase">{{ file.container }}</span>
                  <span v-if="file.file_size_bytes" class="font-mono">
                    {{ formatBytes(file.file_size_bytes) }}
                  </span>
                  <span v-if="file.subtitles?.length" class="text-emerald-300">
                    外挂字幕 {{ file.subtitles.length }} 条
                  </span>
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- 底部快捷处置操作 -->
        <div class="px-5 py-3.5 border-t border-slate-800 bg-slate-950/80 flex flex-wrap items-center gap-2">
          <button
            v-if="folder.hint_tmdb_id"
            type="button"
            @click="emit('accept-hint', folder)"
            :disabled="busy"
            class="cursor-pointer px-3 py-1.5 rounded-xl bg-sky-600/25 hover:bg-sky-600/40 border border-sky-500/40 text-sky-200 text-xs font-medium flex items-center gap-1.5 disabled:opacity-50"
          >
            <CheckCircle2 class="w-3.5 h-3.5" aria-hidden="true" />
            <span>采纳 TMDB #{{ folder.hint_tmdb_id }} 发布</span>
          </button>

          <button
            v-if="folder.hint_tmdb_id && folder.media_type !== 'movie'"
            type="button"
            @click="emit('accept-hint-movie', folder)"
            :disabled="busy"
            class="cursor-pointer px-3 py-1.5 rounded-xl bg-amber-600/20 hover:bg-amber-600/35 border border-amber-500/40 text-amber-200 text-xs font-medium flex items-center gap-1.5 disabled:opacity-50"
          >
            <Film class="w-3.5 h-3.5" aria-hidden="true" />
            <span>按电影绑定 #{{ folder.hint_tmdb_id }}</span>
          </button>

          <button
            type="button"
            @click="emit('scrape', folder)"
            :disabled="busy"
            class="cursor-pointer px-3 py-1.5 rounded-xl bg-emerald-600/20 hover:bg-emerald-600/35 border border-emerald-500/35 text-emerald-300 text-xs font-medium flex items-center gap-1.5 disabled:opacity-50"
          >
            <Sparkles class="w-3.5 h-3.5" :class="{ 'animate-spin': busy }" aria-hidden="true" />
            <span>立即重新刮削</span>
          </button>

          <button
            type="button"
            @click="emit('open-bind', folder)"
            class="cursor-pointer px-3 py-1.5 rounded-xl bg-indigo-600/20 hover:bg-indigo-600/35 border border-indigo-500/35 text-indigo-300 text-xs font-medium flex items-center gap-1.5"
          >
            <Link2 class="w-3.5 h-3.5" aria-hidden="true" />
            <span>手动绑定 TMDB</span>
          </button>

          <button
            type="button"
            @click="emit('publish-without-tmdb', folder)"
            :disabled="busy"
            class="cursor-pointer px-3 py-1.5 rounded-xl bg-slate-800 hover:bg-slate-700 border border-slate-700 text-slate-200 text-xs font-medium flex items-center gap-1.5 disabled:opacity-50"
          >
            <Send class="w-3.5 h-3.5" aria-hidden="true" />
            <span>无 TMDB 发布</span>
          </button>
        </div>
      </aside>
    </div>
  </Teleport>
</template>

<script setup>
import { computed } from 'vue'
import { X, ExternalLink, CheckCircle2, Film, Sparkles, Link2, Send } from 'lucide-vue-next'

const props = defineProps({
  folder: { type: Object, default: null },
  files: { type: Array, default: () => [] },
  filesLoading: { type: Boolean, default: false },
  busy: { type: Boolean, default: false }
})

const emit = defineEmits([
  'close',
  'scrape',
  'open-bind',
  'accept-hint',
  'accept-hint-movie',
  'publish-without-tmdb'
])

const hasS0MultiFiles = computed(() => {
  return props.files.filter((f) => f.season_number === 0).length > 1
})

function stateLabel(state) {
  switch (state) {
    case 'conflict':
      return '校验存疑'
    case 'failed':
      return '未匹配'
    case 'pending':
      return '待刮削'
    case 'verified':
      return '已校验'
    default:
      return state
  }
}

function tmdbUrl(folder) {
  const kind = folder.media_type === 'tv' ? 'tv' : 'movie'
  return `https://www.themoviedb.org/${kind}/${folder.hint_tmdb_id}`
}

function formatBytes(bytes) {
  const gb = bytes / (1024 * 1024 * 1024)
  if (gb >= 1) return `${gb.toFixed(2)} GB`
  return `${(bytes / (1024 * 1024)).toFixed(0)} MB`
}
</script>
