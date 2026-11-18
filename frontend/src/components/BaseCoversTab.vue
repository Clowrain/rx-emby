<template>
  <section class="space-y-6">
    <div class="card p-6 rounded-2xl flex flex-wrap gap-4 items-center justify-between">
      <div>
        <h2 class="text-base font-bold text-white flex items-center gap-2">
          <ImageIcon class="w-4 h-4 text-emerald-400" aria-hidden="true" />
          <span>基础媒体库客户端封面</span>
        </h2>
        <p class="text-xs text-slate-400 mt-1">
          自定义电影、电视剧、动漫、纪录片与综艺的 16:9 客户端首页封面，支持动态剧照拼贴或内置精选图。
        </p>
      </div>
      <div class="flex items-center gap-3">
        <span class="text-xs text-slate-400">动态封面默认缓存 24 小时，按最新发布时间更新</span>
        <button
          type="button"
          :disabled="refreshingCovers"
          @click="handleRefreshDynamicCovers"
          class="cursor-pointer disabled:opacity-50 px-3 py-1.5 rounded-xl bg-slate-800 hover:bg-slate-700 border border-slate-700 text-xs font-medium text-slate-200 flex items-center gap-1.5 transition-colors"
        >
          <RefreshCw class="w-3.5 h-3.5 text-emerald-400" :class="refreshingCovers ? 'animate-spin' : ''" aria-hidden="true" />
          <span>{{ refreshingCovers ? '刷新中…' : '刷新动态封面' }}</span>
        </button>
      </div>
    </div>

    <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-5 gap-4">
      <div
        v-for="lib in baseLibraryCovers"
        :key="lib.id"
        class="card card-interactive rounded-2xl overflow-hidden flex flex-col justify-between"
      >
        <button
          type="button"
          :aria-label="`设置 ${lib.name} 客户端封面`"
          @click="openBaseCoverModal(lib)"
          class="cursor-pointer text-left group relative bg-slate-950 aspect-video overflow-hidden"
        >
          <img
            :src="coverImageUrl(lib.id, lib.cover_image, lib.id)"
            :alt="lib.name"
            class="w-full h-full object-cover transition-transform duration-300 group-hover:scale-105"
          />
          <div
            v-if="isDynamicCover(lib.cover_image)"
            class="absolute bottom-2 left-2 px-2 py-0.5 rounded-md bg-slate-950/85 backdrop-blur-xs border border-white/10 text-[11px] font-medium text-amber-300"
          >
            {{ dynamicCoverLabel(lib.cover_image) }}
          </div>
        </button>

        <div class="px-3.5 py-3 flex items-center justify-between gap-2">
          <div class="min-w-0">
            <div class="font-bold text-sm text-white truncate">{{ lib.name }}</div>
            <div class="text-[11px] text-slate-400 truncate">
              {{ coverStyleSummary(lib.cover_image) }}
            </div>
          </div>
          <button
            type="button"
            @click="openBaseCoverModal(lib)"
            class="cursor-pointer shrink-0 px-3 py-1.5 rounded-xl bg-slate-800 hover:bg-slate-700 text-xs font-medium text-emerald-300 transition-colors inline-flex items-center gap-1.5"
          >
            <Pencil class="w-3.5 h-3.5" aria-hidden="true" />
            <span>更换封面</span>
          </button>
        </div>
      </div>
    </div>

    <!-- 基础媒体库封面编辑弹窗 -->
    <div
      v-if="baseModal.visible"
      role="dialog"
      aria-modal="true"
      aria-labelledby="base-cover-modal-title"
      class="fixed inset-0 bg-black/75 backdrop-blur-xs flex items-center justify-center z-50 p-4"
      @click.self="!baseSaving && (baseModal.visible = false)"
    >
      <div class="card p-6 rounded-2xl max-w-2xl w-full space-y-4 max-h-[90vh] overflow-y-auto">
        <div class="flex items-center justify-between border-b border-slate-800 pb-3">
          <h3 id="base-cover-modal-title" class="text-base font-bold text-white flex items-center gap-2">
            <ImageIcon class="w-4 h-4 text-emerald-400" aria-hidden="true" />
            <span>设置基础媒体库封面：{{ baseModal.name }}</span>
          </h3>
          <button
            type="button"
            aria-label="关闭封面设置弹窗"
            @click="baseModal.visible = false"
            class="cursor-pointer text-slate-400 hover:text-white p-1 rounded-lg hover:bg-slate-800 transition-colors"
          >
            <X class="w-4 h-4" aria-hidden="true" />
          </button>
        </div>

        <div class="space-y-3 text-xs">
          <div class="grid grid-cols-2 sm:grid-cols-3 gap-2">
            <button
              type="button"
              @click="baseModal.coverImage = ''"
              class="cursor-pointer rounded-xl border p-2 text-left transition-all"
              :class="
                !baseModal.coverImage
                  ? 'border-emerald-500 bg-emerald-950/40 text-emerald-200'
                  : 'border-slate-800 bg-slate-950/70 text-slate-400 hover:border-slate-700'
              "
            >
              <div class="text-[11px] font-semibold">默认封面</div>
              <div class="text-[10px] text-slate-500 mt-0.5">恢复内置默认图</div>
            </button>
            <button
              type="button"
              @click="baseModal.coverImage = 'auto'"
              class="cursor-pointer rounded-xl border p-2 text-left transition-all"
              :class="
                baseModal.coverImage === 'auto'
                  ? 'border-amber-500 bg-amber-950/40 text-amber-200'
                  : 'border-slate-800 bg-slate-950/70 text-slate-400 hover:border-slate-700'
              "
            >
              <div class="text-[11px] font-semibold">动态首图</div>
              <div class="text-[10px] text-slate-500 mt-0.5">取该库高分匹配剧照</div>
            </button>
            <button
              v-for="s in collageOptions"
              :key="s.key"
              type="button"
              @click="baseModal.coverImage = s.key"
              class="cursor-pointer rounded-xl border p-2 text-left transition-all"
              :class="
                baseModal.coverImage === s.key
                  ? 'border-indigo-400 bg-indigo-950/40 text-indigo-200'
                  : 'border-slate-800 bg-slate-950/70 text-slate-400 hover:border-slate-700'
              "
            >
              <div class="text-[11px] font-semibold">{{ s.label }}</div>
              <div class="text-[10px] text-slate-500 mt-0.5">{{ s.desc }}</div>
            </button>
          </div>

          <div class="grid grid-cols-3 sm:grid-cols-4 gap-2 pt-1">
            <button
              v-for="c in coverOptions"
              :key="c.key"
              type="button"
              @click="baseModal.coverImage = c.key"
              class="cursor-pointer relative rounded-xl overflow-hidden border aspect-video transition-all"
              :class="
                baseModal.coverImage === c.key
                  ? 'border-emerald-400 ring-2 ring-emerald-500/40'
                  : 'border-slate-800 hover:border-slate-600'
              "
            >
              <img :src="`/covers/${c.key}.jpg`" :alt="c.label" class="w-full h-full object-cover" />
              <div class="absolute inset-x-0 bottom-0 bg-black/75 px-1.5 py-0.5 text-[10px] text-slate-200 truncate">
                {{ c.label }}
              </div>
            </button>
          </div>
        </div>

        <div class="flex justify-end gap-2.5 pt-3 border-t border-slate-800">
          <button
            type="button"
            @click="baseModal.visible = false"
            class="cursor-pointer bg-slate-800 hover:bg-slate-700 px-4 py-2 rounded-xl text-xs font-medium transition-colors"
          >
            取消
          </button>
          <button
            type="button"
            @click="submitBaseCoverModal" :disabled="baseSaving"
            class="cursor-pointer bg-emerald-600 hover:bg-emerald-500 px-5 py-2 rounded-xl text-xs font-semibold text-white transition-colors disabled:opacity-50"
          >
            {{ baseSaving ? '正在保存…' : '保存并生效' }}
          </button>
        </div>
      </div>
    </div>
  </section>
</template>

<script setup lang="ts">
import { ref, reactive, onMounted } from 'vue'
import { Image as ImageIcon, Pencil, X, RefreshCw } from 'lucide-vue-next'
import { adminApi, embyImageUrl } from '../api/admin'
import { useToast } from '../composables/useToast'

const toast = useToast()
const baseSaving = ref(false)
const refreshingCovers = ref(false)
const coverBust = ref(Date.now())

const baseLibraryCovers = ref([
  { id: 'lib_movie', name: '电影', cover_image: null },
  { id: 'lib_tv', name: '电视剧', cover_image: null },
  { id: 'lib_anime', name: '动漫', cover_image: null },
  { id: 'lib_doc', name: '纪录片', cover_image: null },
  { id: 'lib_variety', name: '综艺', cover_image: null }
])

const baseModal = reactive({
  visible: false,
  id: '',
  name: '',
  coverImage: ''
})

const collageOptions = [
  { key: 'collage_cards', label: '立体叠放卡片', desc: '模糊底图 + 微倾卡片' },
  { key: 'collage_diagonal', label: '斜切电影条带', desc: '4图13°斜切 + 暗角' },
  { key: 'collage_hero', label: '1主3辅非对称', desc: '左主剧照 + 右3缩略图' },
  { key: 'collage_blended', label: '羽化融合4连', desc: '4海报渐变无缝拼接' }
]

const coverOptions = [
  { key: 'vlib_top_rated', label: '高分佳作' },
  { key: 'vlib_streaming', label: '流媒体剧场' },
  { key: 'vlib_chinese', label: '华语热播' },
  { key: 'vlib_kr_jp', label: '日韩精选' },
  { key: 'vlib_latest', label: '最新院线' },
  { key: 'vlib_default_movies', label: '电影视图' },
  { key: 'vlib_default_tv', label: '剧集视图' },
  { key: 'lib_movie', label: '影院胶片' },
  { key: 'lib_tv', label: '电视光影' },
  { key: 'lib_anime', label: '二次元星空' },
  { key: 'lib_doc', label: '自然山海' },
  { key: 'lib_variety', label: '舞台灯光' }
]

function isDynamicCover(val: any) {
  return val === 'auto' || (typeof val === 'string' && val.startsWith('collage_'))
}

function dynamicCoverLabel(val: any) {
  if (val === 'auto') return '动态首图'
  const found = collageOptions.find(o => o.key === val)
  return found ? found.label : '动态拼贴'
}

function coverStyleSummary(val: any) {
  if (!val) return '内置默认封面'
  if (isDynamicCover(val)) return dynamicCoverLabel(val)
  const found = coverOptions.find(o => o.key === val)
  return found ? `内置：${found.label}` : '自定义封面'
}

function coverImageUrl(viewId: string, coverImage: any, fallbackKey: string) {
  if (isDynamicCover(coverImage)) {
    return embyImageUrl(`/Items/${viewId}/Images/Primary?tag=${encodeURIComponent(coverImage)}&_t=${coverBust.value}`)
  }
  const key = coverImage || fallbackKey
  return `/covers/${key}.jpg?_t=${coverBust.value}`
}

function openBaseCoverModal(lib: any) {
  baseModal.id = lib.id
  baseModal.name = lib.name
  baseModal.coverImage = lib.cover_image || ''
  baseModal.visible = true
}

async function loadBaseLibraries() {
  try {
    const data = await adminApi.listBaseLibraryCovers()
    if (Array.isArray(data.libraries) && data.libraries.length > 0) {
      baseLibraryCovers.value = data.libraries
    }
  } catch {
    // 忽略初始加载失败，保留默认基础库列表
  }
}

async function submitBaseCoverModal() {
  if (baseSaving.value) return
  baseSaving.value = true
  try {
    const res = await adminApi.saveBaseLibraryCover({
      id: baseModal.id,
      cover_image: baseModal.coverImage || null
    })
    toast.success(res.message)
    baseModal.visible = false
    coverBust.value = Date.now()
    await loadBaseLibraries()
  } catch (err: any) {
    toast.error('保存失败：' + err.message)
  } finally {
    baseSaving.value = false
  }
}

async function handleRefreshDynamicCovers() {
  if (refreshingCovers.value) return
  refreshingCovers.value = true
  try {
    const res = await adminApi.refreshDynamicCovers()
    coverBust.value = Date.now()
    toast.success(res.message)
  } catch (err: any) {
    toast.error('刷新失败：' + err.message)
  } finally {
    refreshingCovers.value = false
  }
}

onMounted(loadBaseLibraries)
</script>
