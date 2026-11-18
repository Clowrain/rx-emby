<template>
  <div
    v-if="mediaId"
    role="dialog"
    aria-modal="true"
    aria-labelledby="drawer-media-title"
    class="fixed inset-0 bg-black/75 backdrop-blur-xs z-50 flex justify-end"
  >
    <div @click="$emit('close')" class="flex-1"></div>
    <div class="w-full max-w-3xl bg-[#0b101b] border-l border-slate-800 h-full flex flex-col shadow-2xl">
      <div class="px-6 py-4 border-b border-slate-800 flex items-center justify-between bg-[#080c16]/90">
        <div class="flex items-center gap-3 min-w-0">
          <h2 id="drawer-media-title" class="text-lg font-bold text-white truncate">
            {{ media ? `${media.title} ${media.year ? `(${media.year})` : ''}` : '影视详情与多版本源明细' }}
          </h2>
          <span
            v-if="media"
            class="shrink-0 text-xs px-2.5 py-0.5 rounded-md bg-emerald-950 text-emerald-300 border border-emerald-500/30"
          >
            {{ media.category }} · {{ media.media_type === 'movie' ? '电影' : '剧集' }}
          </span>
          <span
            v-if="media && (media.is_locked || !media.tmdb_id)"
            class="shrink-0 text-xs px-2 py-0.5 rounded-md bg-amber-950/80 text-amber-300 border border-amber-500/35"
          >
            {{ media.tmdb_id ? '已锁定' : '手动维护' }}
          </span>
        </div>
        <div class="flex items-center gap-2 shrink-0">
          <button
            v-if="media"
            type="button"
            @click="toggleEditMetadata"
            class="cursor-pointer bg-indigo-600/25 hover:bg-indigo-600/40 border border-indigo-500/40 px-3 py-1.5 rounded-xl text-xs font-medium text-indigo-200 transition-colors flex items-center gap-1.5"
          >
            <Pencil class="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
            <span>{{ editingMetadata ? '收起编辑' : '编辑信息' }}</span>
          </button>
          <button
            v-if="media && media.tmdb_id"
            type="button"
            @click="handleUnbindTmdb(false)"
            :disabled="savingMetadata"
            class="cursor-pointer bg-amber-500/15 hover:bg-amber-500/25 border border-amber-500/35 disabled:opacity-50 px-3 py-1.5 rounded-xl text-xs font-medium text-amber-200 transition-colors flex items-center gap-1.5"
            title="取消 TMDB 关联并转为手动维护（仍保留在海报墙）"
          >
            <Unlink class="w-3.5 h-3.5 shrink-0" aria-hidden="true" />
            <span>取消刮削</span>
          </button>
          <button
            type="button"
            @click="handlePrewarmDirs"
            :disabled="prewarming"
            class="cursor-pointer bg-sky-600/25 hover:bg-sky-600/40 border border-sky-500/40 disabled:opacity-50 px-3.5 py-1.5 rounded-xl text-xs font-medium text-sky-200 transition-colors flex items-center gap-1.5"
          >
            <Zap class="w-3.5 h-3.5 text-sky-300 shrink-0" aria-hidden="true" />
            <span>{{ prewarming ? '正在获取目录与文件大小…' : '预热目录并获取文件大小' }}</span>
          </button>
          <button
            type="button"
            aria-label="关闭详情抽屉"
            @click="$emit('close')"
            class="cursor-pointer bg-slate-800 hover:bg-slate-700 px-3 py-1.5 rounded-xl text-xs text-slate-300 flex items-center gap-1 transition-colors"
          >
            <X class="w-3.5 h-3.5" aria-hidden="true" />
            <span>关闭</span>
          </button>
        </div>
      </div>

      <div class="flex-1 overflow-y-auto p-6 space-y-6">
        <div v-if="loading" class="text-center py-12 text-slate-400 text-xs">
          正在加载分集与多版本源明细…
        </div>
        <template v-else-if="media">
          <!-- 基础元数据卡片 -->
          <div class="flex gap-4 bg-slate-950/60 p-4 rounded-2xl border border-slate-800/90">
            <img
              v-if="media.poster_path"
              :src="embyImageUrl(`/emby/Items/work_${media.id}/Images/Primary`)"
              :alt="`${media.title} 海报`"
              width="96"
              height="144"
              class="w-24 h-36 object-cover rounded-xl shrink-0 shadow-lg"
            />
            <div class="space-y-2.5 text-sm">
              <div class="flex flex-wrap items-center gap-2 tabular-nums">
                <span class="text-amber-400 font-bold flex items-center gap-1">
                  <Star class="w-3.5 h-3.5 fill-amber-400 text-amber-400" aria-hidden="true" />
                  <span>{{ media.rating ? media.rating.toFixed(1) : '0.0' }}</span>
                </span>
                <span
                  v-if="media.tmdb_id"
                  class="px-2 py-0.5 rounded-md bg-slate-800 text-xs font-mono text-emerald-400"
                >
                  TMDB #{{ media.tmdb_id }}
                </span>
                <span v-if="media.original_title" class="text-xs text-slate-400">
                  {{ media.original_title }}
                </span>
              </div>
              <div class="flex flex-wrap gap-1.5">
                <span
                  v-for="c in media.countries || []"
                  :key="'c-' + c"
                  class="px-2 py-0.5 rounded-md bg-slate-800 text-slate-300 text-xs"
                >
                  {{ c }}
                </span>
                <span
                  v-for="n in media.networks || []"
                  :key="'n-' + n"
                  class="px-2 py-0.5 rounded-md bg-indigo-950 text-indigo-300 border border-indigo-700/40 text-xs"
                >
                  {{ n }}
                </span>
                <span
                  v-for="g in media.genres || []"
                  :key="'g-' + g"
                  class="px-2 py-0.5 rounded-md bg-emerald-950 text-emerald-300 border border-emerald-700/40 text-xs"
                >
                  {{ g }}
                </span>
              </div>
              <p class="text-xs text-slate-300 leading-relaxed max-w-[68ch]">
                {{ media.overview || '暂无剧情简介' }}
              </p>
            </div>
          </div>

          <!-- 手动维护元数据表单 -->
          <form
            v-if="editingMetadata"
            @submit.prevent="submitManualMetadata"
            class="bg-slate-950/80 border border-indigo-500/40 rounded-2xl p-4 space-y-3 text-xs"
          >
            <div class="flex items-center justify-between gap-2">
              <span class="font-semibold text-indigo-200">手动维护作品信息（保存后自动锁定，不被后台刮削覆盖）</span>
              <button
                type="button"
                @click="handleUnbindTmdb(true)"
                :disabled="savingMetadata"
                class="cursor-pointer text-[11px] text-rose-300 hover:underline disabled:opacity-50"
              >
                退回待刮削中间表
              </button>
            </div>

            <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
              <label class="space-y-1 block">
                <span class="text-[11px] text-slate-400">片名 *</span>
                <input
                  v-model="metaForm.title"
                  required
                  class="w-full bg-slate-900 border border-slate-700 rounded-lg px-2.5 py-1.5 text-white focus:outline-none focus:border-indigo-500"
                />
              </label>
              <label class="space-y-1 block">
                <span class="text-[11px] text-slate-400">原名 / 英文名</span>
                <input
                  v-model="metaForm.original_title"
                  class="w-full bg-slate-900 border border-slate-700 rounded-lg px-2.5 py-1.5 text-white focus:outline-none focus:border-indigo-500"
                />
              </label>
              <label class="space-y-1 block">
                <span class="text-[11px] text-slate-400">年份</span>
                <input
                  v-model="metaForm.year"
                  type="number"
                  placeholder="例如 2026"
                  class="w-full bg-slate-900 border border-slate-700 rounded-lg px-2.5 py-1.5 text-white font-mono focus:outline-none focus:border-indigo-500"
                />
              </label>
              <label class="space-y-1 block">
                <span class="text-[11px] text-slate-400">评分 (0-10)</span>
                <input
                  v-model="metaForm.rating"
                  type="number"
                  step="0.1"
                  min="0"
                  max="10"
                  class="w-full bg-slate-900 border border-slate-700 rounded-lg px-2.5 py-1.5 text-white font-mono focus:outline-none focus:border-indigo-500"
                />
              </label>
              <label class="space-y-1 block">
                <span class="text-[11px] text-slate-400">媒体类型</span>
                <select
                  v-model="metaForm.media_type"
                  class="w-full bg-slate-900 border border-slate-700 rounded-lg px-2.5 py-1.5 text-white focus:outline-none focus:border-indigo-500"
                >
                  <option value="movie">电影 (movie)</option>
                  <option value="tv">剧集 (tv)</option>
                </select>
              </label>
              <label class="space-y-1 block">
                <span class="text-[11px] text-slate-400">所属分类</span>
                <select
                  v-model="metaForm.category"
                  class="w-full bg-slate-900 border border-slate-700 rounded-lg px-2.5 py-1.5 text-white focus:outline-none focus:border-indigo-500"
                >
                  <option value="电影">电影</option>
                  <option value="电视剧">电视剧</option>
                  <option value="动漫">动漫</option>
                  <option value="纪录片">纪录片</option>
                  <option value="综艺">综艺</option>
                </select>
              </label>
              <label class="space-y-1 block">
                <span class="text-[11px] text-slate-400">类型标签（逗号分隔）</span>
                <input
                  v-model="metaForm.genres"
                  placeholder="剧情, 悬疑"
                  class="w-full bg-slate-900 border border-slate-700 rounded-lg px-2.5 py-1.5 text-white focus:outline-none focus:border-indigo-500"
                />
              </label>
              <label class="space-y-1 block">
                <span class="text-[11px] text-slate-400">国家/地区（逗号分隔）</span>
                <input
                  v-model="metaForm.countries"
                  placeholder="中国大陆"
                  class="w-full bg-slate-900 border border-slate-700 rounded-lg px-2.5 py-1.5 text-white focus:outline-none focus:border-indigo-500"
                />
              </label>
            </div>

            <label class="space-y-1 block">
              <span class="text-[11px] text-slate-400">海报路径（TMDB path 如 /xxx.jpg，留空则显示默认占位）</span>
              <input
                v-model="metaForm.poster_path"
                class="w-full bg-slate-900 border border-slate-700 rounded-lg px-2.5 py-1.5 text-white font-mono focus:outline-none focus:border-indigo-500"
              />
            </label>

            <label class="space-y-1 block">
              <span class="text-[11px] text-slate-400">剧情简介</span>
              <textarea
                v-model="metaForm.overview"
                rows="3"
                class="w-full bg-slate-900 border border-slate-700 rounded-lg px-2.5 py-1.5 text-white focus:outline-none focus:border-indigo-500"
              ></textarea>
            </label>

            <div class="flex flex-wrap items-center justify-between gap-3 pt-1">
              <div class="flex flex-wrap items-center gap-4">
                <label class="inline-flex items-center gap-1.5 text-slate-300 cursor-pointer">
                  <input v-model="metaForm.is_locked" type="checkbox" class="rounded border-slate-600" />
                  <span>锁定元数据（禁止自动刮削覆盖）</span>
                </label>
                <label v-if="media.tmdb_id" class="inline-flex items-center gap-1.5 text-amber-300 cursor-pointer">
                  <input v-model="metaForm.detach_tmdb" type="checkbox" class="rounded border-slate-600" />
                  <span>同时解除 TMDB #{{ media.tmdb_id }} 关联</span>
                </label>
              </div>
              <div class="flex items-center gap-2">
                <button
                  type="button"
                  @click="editingMetadata = false"
                  class="cursor-pointer px-3 py-1.5 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-300"
                >
                  取消
                </button>
                <button
                  type="submit"
                  :disabled="savingMetadata"
                  class="cursor-pointer px-3.5 py-1.5 rounded-lg bg-indigo-600 hover:bg-indigo-500 disabled:opacity-50 text-white font-medium"
                >
                  {{ savingMetadata ? '保存中…' : '保存手动维护' }}
                </button>
              </div>
            </div>
          </form>

          <!-- 聚合兄弟目录列表 -->
          <div class="space-y-2">
            <div class="text-xs font-semibold text-slate-400 flex items-center gap-1.5 tabular-nums">
              <FolderGit2 class="w-3.5 h-3.5 text-slate-400" aria-hidden="true" />
              <span>已合并的来源目录 ({{ siblingDirs.length }} 个)</span>
            </div>
            <div class="space-y-1.5">
              <div
                v-for="d in siblingDirs"
                :key="d.id"
                class="flex flex-wrap items-center justify-between gap-2 rounded-xl px-3.5 py-2 text-xs tabular-nums transition-colors duration-200 border"
                :class="
                  selectedFolderId === d.id
                    ? 'bg-sky-950/35 border-sky-400/70 ring-1 ring-sky-400/50'
                    : lastEditedFolderId === d.id
                    ? 'bg-emerald-950/30 border-emerald-500/60 ring-1 ring-emerald-500/40'
                    : 'bg-slate-950/70 border-slate-800'
                "
              >
                <div class="truncate flex-1 min-w-0 pr-2 flex items-center gap-2">
                  <button
                    type="button"
                    @click="toggleFolderHighlight(d.id)"
                    :title="selectedFolderId === d.id ? '点击取消定位高亮' : '点击在下方视频源明细中定位该目录'"
                    class="cursor-pointer shrink-0 px-2 py-0.5 rounded-md font-mono text-[11px] font-semibold border transition-colors"
                    :class="
                      selectedFolderId === d.id || lastEditedFolderId === d.id
                        ? 'bg-sky-500/20 text-sky-300 border-sky-400/40'
                        : 'bg-slate-800 text-slate-300 border-slate-700 hover:border-slate-500'
                    "
                  >
                    目录 #{{ d.id }}
                  </button>
                  <span class="font-mono text-slate-200 truncate" :title="d.folder_path">{{ d.folder_path }}</span>
                  <span
                    v-if="lastEditedFolderId === d.id"
                    class="shrink-0 px-1.5 py-0.5 rounded bg-emerald-500/20 text-emerald-300 text-[10px] font-medium"
                  >
                    已调整
                  </span>
                </div>
                <div class="flex items-center gap-2 shrink-0">
                  <div class="flex items-center gap-1 bg-slate-900 border border-slate-700/80 rounded-lg px-2 py-0.5">
                    <span class="text-[11px] text-slate-400">目录加权</span>
                    <button
                      type="button"
                      :disabled="busyFolderId === d.id"
                      @click="adjustFolderPriority(d, (d.priority_boost || 0) - 20)"
                      class="cursor-pointer disabled:opacity-50 px-1.5 py-0.5 rounded bg-slate-800 hover:bg-slate-700 text-slate-300 text-[11px] transition-colors"
                    >
                      -20
                    </button>
                    <span class="font-mono text-emerald-300 w-11 text-center font-semibold">
                      {{ (d.priority_boost || 0) > 0 ? `+${d.priority_boost}` : (d.priority_boost || 0) }}
                    </span>
                    <button
                      type="button"
                      :disabled="busyFolderId === d.id"
                      @click="adjustFolderPriority(d, (d.priority_boost || 0) + 20)"
                      class="cursor-pointer disabled:opacity-50 px-1.5 py-0.5 rounded bg-slate-800 hover:bg-slate-700 text-slate-300 text-[11px] transition-colors"
                    >
                      +20
                    </button>
                    <button
                      v-if="d.priority_boost"
                      type="button"
                      :disabled="busyFolderId === d.id"
                      @click="adjustFolderPriority(d, 0)"
                      class="cursor-pointer disabled:opacity-50 px-1.5 py-0.5 rounded bg-slate-800 hover:bg-slate-700 text-slate-400 hover:text-slate-200 text-[10px] transition-colors"
                    >
                      归零
                    </button>
                  </div>
                  <button
                    v-if="siblingDirs.length > 1 && !d.is_ignored"
                    type="button"
                    :disabled="busyFolderId === d.id"
                    @click="handleSplitFolder(d)"
                    class="cursor-pointer disabled:opacity-50 px-2 py-0.5 rounded-lg bg-amber-500/15 hover:bg-amber-500/25 border border-amber-500/35 text-amber-200 text-[11px] flex items-center gap-1 transition-colors"
                    title="将该目录从当前多版本合并中拆分为独立手动维护条目"
                  >
                    <Scissors class="w-3 h-3" aria-hidden="true" />
                    <span>拆分独立</span>
                  </button>
                  <span class="px-2 py-0.5 rounded-md bg-slate-800 text-emerald-400 font-mono">
                    {{ d.file_count }} 个源
                  </span>
                </div>
              </div>
            </div>
          </div>

          <!-- 分集与多版本源明细 -->
          <div class="space-y-3">
            <div class="text-xs font-semibold text-slate-400 flex items-center gap-1.5 tabular-nums">
              <HardDrive class="w-3.5 h-3.5 text-slate-400" aria-hidden="true" />
              <span>视频源明细 (共 {{ episodes.length }} 个文件 · 已按画质优先级排序)</span>
            </div>
            <div v-if="groupedEpisodes.length === 0" class="text-xs text-slate-500">
              暂无关联视频源
            </div>
            <div
              v-for="grp in groupedEpisodes"
              :key="grp.key"
              class="card rounded-2xl p-4 space-y-3"
            >
              <div class="flex gap-3.5 items-start">
                <img
                  v-if="media.media_type !== 'movie' && grp.rep.still_path"
                  :src="embyImageUrl(`/emby/Items/ep_${media.id}_${grp.rep.season_number}_${grp.rep.episode_number}/Images/Primary`)"
                  :alt="grp.headerLabel"
                  width="128"
                  height="72"
                  loading="lazy"
                  class="w-32 h-18 object-cover rounded-lg shrink-0 border border-slate-800 shadow-sm"
                />
                <div class="flex-1 min-w-0 space-y-1">
                  <div class="flex items-center justify-between gap-2 flex-wrap">
                    <span class="text-sm font-semibold text-slate-100">{{ grp.headerLabel }}</span>
                    <div class="flex items-center gap-1.5 text-[11px] tabular-nums">
                      <span
                        v-if="grp.rep.air_date"
                        class="px-2 py-0.5 rounded bg-slate-800/90 text-slate-300 font-mono"
                      >
                        {{ grp.rep.air_date }}
                      </span>
                      <span
                        v-if="grp.rep.runtime > 0"
                        class="px-2 py-0.5 rounded bg-slate-800/90 text-slate-300 font-mono"
                      >
                        {{ grp.rep.runtime }} 分钟
                      </span>
                      <span
                        v-if="media.media_type !== 'movie' && grp.rep.meta_incomplete"
                        class="px-2 py-0.5 rounded bg-amber-950/80 text-amber-300 border border-amber-500/30"
                        title="新集刚更新或 TMDB 词条暂未完善，将在冷却期后自动重刷补全"
                      >
                        待补全剧照/简介
                      </span>
                    </div>
                  </div>
                  <p
                    v-if="media.media_type !== 'movie' && grp.rep.overview"
                    class="text-xs text-slate-400 leading-relaxed line-clamp-2"
                  >
                    {{ grp.rep.overview }}
                  </p>
                </div>
              </div>
              <div class="space-y-2">
                <div
                  v-for="(src, srcIdx) in grp.list"
                  :key="src.id"
                  class="rounded-xl p-3 space-y-1.5 text-xs tabular-nums transition-colors duration-200 border"
                  :class="
                    lastEditedFileId === src.id
                      ? 'bg-emerald-950/25 border-emerald-500/60 ring-1 ring-emerald-500/40'
                      : activeFolderHighlightId === src.folder_id
                      ? 'bg-sky-950/25 border-sky-400/60 ring-1 ring-sky-400/35'
                      : 'bg-slate-950/80 border-slate-800/80'
                  "
                >
                  <div class="flex items-center justify-between gap-2">
                    <div class="flex items-center gap-2 flex-wrap">
                      <span
                        v-if="srcIdx === 0"
                        class="px-1.5 py-0.5 rounded bg-sky-500/20 text-sky-300 border border-sky-400/30 text-[10px] font-semibold"
                      >
                        默认首播
                      </span>
                      <button
                        type="button"
                        @click="toggleFolderHighlight(src.folder_id)"
                        :title="folderPathById[src.folder_id] || `目录 #${src.folder_id}`"
                        class="cursor-pointer px-1.5 py-0.5 rounded font-mono text-[10px] border transition-colors"
                        :class="
                          activeFolderHighlightId === src.folder_id
                            ? 'bg-sky-500/25 text-sky-200 border-sky-400/50 font-semibold'
                            : 'bg-slate-800/90 text-slate-300 border-slate-700 hover:border-slate-500'
                        "
                      >
                        目录 #{{ src.folder_id }}
                      </button>
                      <span class="px-2 py-0.5 rounded-md bg-emerald-950 text-emerald-300 border border-emerald-500/30 font-medium">
                        {{ src.version_tag }}
                      </span>
                      <span class="px-1.5 py-0.5 rounded bg-slate-800 text-slate-300 uppercase font-mono text-[10px]">
                        {{ src.container }}
                      </span>
                      <span class="text-slate-400 text-[11px] font-mono">
                        有效权重: <strong class="text-emerald-300">{{ src.effective_priority }}</strong>
                        <span v-if="src.folder_priority_boost" class="text-sky-300 ml-1">
                          (目录 {{ src.folder_priority_boost > 0 ? `+${src.folder_priority_boost}` : src.folder_priority_boost }})
                        </span>
                        <span v-if="src.priority_override !== null && src.priority_override !== undefined" class="text-amber-300 ml-1">(已锁定 {{ src.priority_override }})</span>
                      </span>
                      <button
                        type="button"
                        :disabled="busyFileId === src.id"
                        @click="setFilePriority(src, (src.effective_priority || 0) + 100)"
                        class="cursor-pointer disabled:opacity-50 px-1.5 py-0.5 rounded bg-indigo-950/80 hover:bg-indigo-900 text-indigo-300 border border-indigo-500/30 text-[10px]"
                      >
                        置顶(+100)
                      </button>
                      <button
                        v-if="src.priority_override !== null && src.priority_override !== undefined"
                        type="button"
                        :disabled="busyFileId === src.id"
                        @click="setFilePriority(src, null)"
                        class="cursor-pointer disabled:opacity-50 px-1.5 py-0.5 rounded bg-slate-800 hover:bg-slate-700 text-slate-300 text-[10px]"
                      >
                        恢复默认
                      </button>
                    </div>
                    <div>
                      <span v-if="!src.file_size_bytes || src.file_size_bytes <= 0" class="text-slate-500">
                        未获取大小
                      </span>
                      <span v-else class="text-emerald-400 font-mono font-semibold">
                        {{ formatFileSize(src.file_size_bytes) }}
                      </span>
                    </div>
                  </div>
                  <div class="font-mono text-[11px] text-slate-400 break-all">
                    {{ src.alist_path }}
                  </div>
                </div>
              </div>
            </div>
          </div>
        </template>
      </div>
    </div>
  </div>
</template>

<script setup>
import { ref, reactive, computed, watch } from 'vue'
import { Zap, X, Star, FolderGit2, HardDrive, Pencil, Unlink, Scissors } from 'lucide-vue-next'
import { adminApi, embyImageUrl } from '../api/admin'
import { useToast } from '../composables/useToast'

const props = defineProps({
  mediaId: {
    type: Number,
    default: null
  }
})

defineEmits(['close'])

const toast = useToast()
const loading = ref(false)
const prewarming = ref(false)
const savingMetadata = ref(false)
const editingMetadata = ref(false)
const media = ref(null)
const siblingDirs = ref([])
const episodes = ref([])
const busyFolderId = ref(null)
const busyFileId = ref(null)
const lastEditedFolderId = ref(null)
const lastEditedFileId = ref(null)
const selectedFolderId = ref(null)

const metaForm = reactive({
  title: '',
  original_title: '',
  year: '',
  media_type: 'movie',
  category: '电影',
  rating: '0',
  genres: '',
  countries: '',
  poster_path: '',
  overview: '',
  is_locked: true,
  detach_tmdb: false
})

function syncMetaForm(m) {
  if (!m) return
  metaForm.title = m.title || ''
  metaForm.original_title = m.original_title || ''
  metaForm.year = m.year ? String(m.year) : ''
  metaForm.media_type = m.media_type || 'movie'
  metaForm.category = m.category || '电影'
  metaForm.rating = m.rating != null ? String(m.rating) : '0'
  metaForm.genres = (m.genres || []).join(', ')
  metaForm.countries = (m.countries || []).join(', ')
  metaForm.poster_path = m.poster_path || ''
  metaForm.overview = m.overview || ''
  metaForm.is_locked = true
  metaForm.detach_tmdb = false
}

function toggleEditMetadata() {
  if (!editingMetadata.value && media.value) {
    syncMetaForm(media.value)
  }
  editingMetadata.value = !editingMetadata.value
}

async function submitManualMetadata() {
  if (!media.value) return
  savingMetadata.value = true
  try {
    const parseList = (s) =>
      String(s || '')
        .split(/[,，]/)
        .map((x) => x.trim())
        .filter(Boolean)
    const yearNum = metaForm.year ? parseInt(metaForm.year, 10) : null
    const ratingNum = metaForm.rating !== '' ? parseFloat(metaForm.rating) : 0
    const res = await adminApi.updateMediaMetadata(media.value.id, {
      title: metaForm.title,
      original_title: metaForm.original_title || null,
      year: Number.isFinite(yearNum) ? yearNum : null,
      media_type: metaForm.media_type,
      category: metaForm.category,
      rating: Number.isFinite(ratingNum) ? ratingNum : 0,
      genres: parseList(metaForm.genres),
      countries: parseList(metaForm.countries),
      poster_path: metaForm.poster_path || null,
      overview: metaForm.overview || null,
      is_locked: metaForm.is_locked,
      detach_tmdb: metaForm.detach_tmdb
    })
    toast.success(res.message)
    editingMetadata.value = false
    await fetchDetail(media.value.id, { silent: true })
  } catch (err) {
    toast.error('保存失败：' + err.message)
  } finally {
    savingMetadata.value = false
  }
}

async function handleUnbindTmdb(resetToStaging = false) {
  if (!media.value) return
  savingMetadata.value = true
  try {
    const res = await adminApi.unbindMediaTmdb(media.value.id, {
      clear_poster: false,
      reset_to_staging: resetToStaging
    })
    toast.success(res.message)
    if (!resetToStaging) {
      await fetchDetail(media.value.id, { silent: true })
      syncMetaForm(media.value)
      editingMetadata.value = true
    } else {
      editingMetadata.value = false
      await fetchDetail(media.value.id, { silent: true })
    }
  } catch (err) {
    toast.error('操作失败：' + err.message)
  } finally {
    savingMetadata.value = false
  }
}

async function handleSplitFolder(folder) {
  busyFolderId.value = folder.id
  try {
    const res = await adminApi.splitFolder(folder.id)
    toast.success(`${res.message}（新作品 #${res.new_work_id}）`)
    if (media.value) {
      await fetchDetail(media.value.id, { silent: true })
    }
  } catch (err) {
    toast.error('拆分失败：' + err.message)
  } finally {
    busyFolderId.value = null
  }
}

const activeFolderHighlightId = computed(() => selectedFolderId.value ?? lastEditedFolderId.value)

const folderPathById = computed(() => {
  const map = {}
  for (const d of siblingDirs.value) {
    map[d.id] = d.folder_path
  }
  return map
})

function toggleFolderHighlight(folderId) {
  selectedFolderId.value = selectedFolderId.value === folderId ? null : folderId
}

const groupedEpisodes = computed(() => {
  if (!media.value) return []
  const map = new Map()
  for (const ep of episodes.value) {
    const key = `S${String(ep.season_number).padStart(2, '0')}E${String(ep.episode_number).padStart(2, '0')}`
    if (!map.has(key)) map.set(key, [])
    map.get(key).push(ep)
  }
  const result = []
  for (const [key, list] of map.entries()) {
    const rep = list.find(item => item.still_path || item.overview) || list[0]
    const headerLabel =
      media.value.media_type === 'movie'
        ? `电影正片版本 (${list.length} 个源)`
        : `${key} · ${rep.title || `第 ${rep.episode_number} 集`} (${list.length} 个版本)`
    result.push({ key, headerLabel, rep, list })
  }
  return result
})

function formatFileSize(bytes) {
  if (bytes >= 1073741824) return `${(bytes / 1073741824).toFixed(2)} GB`
  return `${(bytes / 1048576).toFixed(1)} MB`
}

async function fetchDetail(id, { silent = false } = {}) {
  if (!id) return
  if (!silent && (!media.value || media.value.id !== id)) {
    loading.value = true
  }
  try {
    const data = await adminApi.getMediaDetail(id)
    media.value = data.media
    siblingDirs.value = data.folders || data.sibling_dirs || []
    episodes.value = data.episodes || []
  } catch (err) {
    toast.error('加载详情失败：' + err.message)
  } finally {
    loading.value = false
  }
}

async function adjustFolderPriority(folder, nextVal) {
  if (busyFolderId.value === folder.id) return
  const prevBoost = folder.priority_boost || 0
  busyFolderId.value = folder.id
  lastEditedFolderId.value = folder.id
  folder.priority_boost = nextVal
  try {
    const res = await adminApi.updateFolderPriority(folder.id, nextVal)
    toast.success(res.message)
    if (props.mediaId) await fetchDetail(props.mediaId, { silent: true })
  } catch (err) {
    folder.priority_boost = prevBoost
    toast.error('调整目录权重失败：' + err.message)
  } finally {
    busyFolderId.value = null
  }
}

async function setFilePriority(file, overrideVal) {
  if (busyFileId.value === file.id) return
  busyFileId.value = file.id
  lastEditedFileId.value = file.id
  try {
    const res = await adminApi.updateFilePriority(file.id, overrideVal)
    toast.success(res.message)
    if (props.mediaId) await fetchDetail(props.mediaId, { silent: true })
  } catch (err) {
    toast.error('调整文件权重失败：' + err.message)
  } finally {
    busyFileId.value = null
  }
}

async function handlePrewarmDirs() {
  if (!props.mediaId) return
  prewarming.value = true
  try {
    const res = await adminApi.prewarmMediaDirs(props.mediaId)
    toast.info(res.message)
    setTimeout(async () => {
      prewarming.value = false
      if (props.mediaId) await fetchDetail(props.mediaId, { silent: true })
    }, 1500)
  } catch (err) {
    prewarming.value = false
    toast.error('目录探测请求失败：' + err.message)
  }
}

watch(
  () => props.mediaId,
  newId => {
    lastEditedFolderId.value = null
    lastEditedFileId.value = null
    selectedFolderId.value = null
    if (newId) {
      fetchDetail(newId)
    } else {
      media.value = null
      siblingDirs.value = []
      episodes.value = []
    }
  },
  { immediate: true }
)
</script>
