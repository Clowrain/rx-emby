<template>
  <section class="space-y-6">
    <!-- 1. 实时活跃播放会话监控 -->
    <div class="card p-6 rounded-2xl">
      <div class="flex items-center justify-between mb-4">
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
          @click="loadAll"
          class="cursor-pointer bg-slate-800 hover:bg-slate-700 px-3.5 py-1.5 rounded-xl text-xs font-medium transition-colors flex items-center gap-1.5"
        >
          <RefreshCw class="w-3.5 h-3.5 text-slate-300" aria-hidden="true" />
          <span>刷新状态</span>
        </button>
      </div>

      <div
        v-if="activeSessions.length === 0"
        class="bg-slate-950/60 border border-slate-800/80 rounded-xl p-6 text-center text-xs text-slate-500"
      >
        当前没有正在播放的会话
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

    <!-- 2. 多用户账号与分类访问权限 -->
    <div class="card rounded-2xl overflow-hidden">
      <div class="px-6 py-4 border-b border-slate-800 flex flex-wrap gap-3 items-center justify-between">
        <div>
          <h2 id="users-table-heading" class="text-base font-bold text-white flex items-center gap-2">
            <Users class="w-4 h-4 text-emerald-400" aria-hidden="true" />
            <span>用户账号与访问权限</span>
          </h2>
          <p class="text-xs text-slate-400 mt-0.5">
            支持为家庭成员或儿童账号配置独立密码与可见媒体库分类范围（如儿童账号仅勾选「动漫」「纪录片」）。
          </p>
        </div>
        <button
          type="button"
          @click="openModal()"
          class="cursor-pointer bg-emerald-600 hover:bg-emerald-500 text-white px-4 py-2 rounded-xl text-xs font-semibold transition-colors flex items-center gap-1.5"
        >
          <UserPlus class="w-3.5 h-3.5" aria-hidden="true" />
          <span>创建新用户</span>
        </button>
      </div>
      <div tabindex="0" role="region" aria-labelledby="users-table-heading" class="table-wrapper">
        <table
          class="w-full text-sm text-left"
          style="--label-2: '角色权限'; --label-3: '认证策略'; --label-4: '授权可见分类'; --label-5: '续播与收藏'; --label-6: '操作'"
        >
          <caption class="visually-hidden">用户账号与访问权限</caption>
          <thead class="bg-[#090d16] text-xs text-slate-400 border-b border-slate-800">
            <tr>
              <th scope="col" class="px-6 py-3.5 font-semibold whitespace-nowrap">账号名称</th>
              <th scope="col" class="px-4 py-3.5 font-semibold whitespace-nowrap">角色权限</th>
              <th scope="col" class="px-4 py-3.5 font-semibold whitespace-nowrap">认证策略</th>
              <th scope="col" class="px-4 py-3.5 font-semibold whitespace-nowrap">授权可见的媒体分类</th>
              <th scope="col" class="px-4 py-3.5 font-semibold whitespace-nowrap">续播历史 / 收藏</th>
              <th scope="col" class="px-6 py-3.5 font-semibold whitespace-nowrap text-right">操作</th>
            </tr>
          </thead>
          <tbody class="divide-y divide-slate-800/70">
            <tr v-for="u in users" :key="u.id" class="hover:bg-slate-900/40 transition-colors">
              <th scope="row" class="px-6 py-3.5 font-normal whitespace-nowrap">
                <div class="flex items-center gap-3">
                  <div class="w-8 h-8 rounded-lg bg-gradient-to-br from-slate-700 to-slate-800 border border-slate-600 flex items-center justify-center text-xs font-bold text-emerald-300 uppercase shrink-0">
                    {{ u.username.slice(0, 2) }}
                  </div>
                  <div>
                    <div class="font-semibold text-white text-sm">{{ u.username }}</div>
                    <div class="text-[11px] text-slate-500 font-mono">ID: {{ u.id }}</div>
                  </div>
                </div>
              </th>
              <td class="px-4 py-3.5 whitespace-nowrap">
                <span
                  v-if="u.is_admin"
                  class="inline-flex items-center gap-1 px-2.5 py-0.5 rounded-md text-xs font-medium bg-emerald-950 text-emerald-300 border border-emerald-500/30"
                >
                  <ShieldCheck class="w-3 h-3" aria-hidden="true" />
                  <span>系统管理员</span>
                </span>
                <span
                  v-else
                  class="inline-flex items-center gap-1 px-2.5 py-0.5 rounded-md text-xs font-medium bg-slate-800 text-slate-300"
                >
                  <User class="w-3 h-3" aria-hidden="true" />
                  <span>家庭成员</span>
                </span>
              </td>
              <td class="px-4 py-3.5 text-xs whitespace-nowrap">
                <span v-if="u.has_password" class="text-emerald-400 inline-flex items-center gap-1.5">
                  <Lock class="w-3.5 h-3.5" aria-hidden="true" />
                  <span>密码保护</span>
                </span>
                <span v-else class="text-amber-400 inline-flex items-center gap-1.5">
                  <Unlock class="w-3.5 h-3.5" aria-hidden="true" />
                  <span>免密快捷登录</span>
                </span>
              </td>
              <td class="px-4 py-3.5 text-xs">
                <span
                  v-for="c in u.allowed_categories || []"
                  :key="c"
                  class="inline-block mr-1.5 mb-1 px-2 py-0.5 rounded-md bg-slate-900 border border-slate-700/80 text-slate-200 whitespace-nowrap"
                >
                  {{ c === '*' ? '全部媒体库' : c === 'vlib_*' ? '所有智能视图' : c }}
                </span>
              </td>
              <td class="px-4 py-3.5 text-xs font-mono text-slate-300 tabular-nums whitespace-nowrap">
                {{ u.history_count }} 条进度 · {{ u.favorite_count }} 收藏
              </td>
              <td class="px-6 py-3.5 text-right whitespace-nowrap">
                <div class="inline-flex items-center gap-3">
                  <button
                    type="button"
                    @click="openModal(u)"
                    :aria-label="`编辑权限 ${u.username}`"
                    class="cursor-pointer text-xs font-medium text-sky-400 hover:text-sky-300 inline-flex items-center gap-1 whitespace-nowrap"
                  >
                    <Pencil class="w-3 h-3 shrink-0" aria-hidden="true" />
                    <span>编辑权限</span>
                  </button>
                  <button
                    type="button"
                    @click="handleDeleteUser(u)"
                    :aria-label="`删除 ${u.username}`"
                    class="cursor-pointer text-xs font-medium text-rose-400 hover:text-rose-300 inline-flex items-center gap-1 whitespace-nowrap"
                  >
                    <Trash2 class="w-3 h-3 shrink-0" aria-hidden="true" />
                    <span>删除</span>
                  </button>
                </div>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>

    <!-- 3. 客户端设备访问令牌 -->
    <div class="card rounded-2xl overflow-hidden">
      <div class="px-6 py-4 border-b border-slate-800 flex items-center justify-between">
        <div>
          <h2 id="tokens-table-heading" class="text-base font-bold text-white flex items-center gap-2">
            <KeyRound class="w-4 h-4 text-emerald-400" aria-hidden="true" />
            <span>已登录客户端设备</span>
          </h2>
          <p class="text-xs text-slate-400 mt-0.5">管理在 VidHub、Infuse、SenPlayer 等客户端登录过的设备会话令牌。</p>
        </div>
      </div>
      <div tabindex="0" role="region" aria-labelledby="tokens-table-heading" class="table-wrapper">
        <table
          class="w-full text-sm text-left"
          style="--label-2: '客户端 / 设备'; --label-3: '最近通信时间'; --label-4: 'Token 摘要'; --label-5: '操作'"
        >
          <caption class="visually-hidden">已登录客户端设备列表</caption>
          <thead class="bg-[#090d16] text-xs text-slate-400 border-b border-slate-800">
            <tr>
              <th scope="col" class="px-6 py-3 font-semibold whitespace-nowrap">所属账号</th>
              <th scope="col" class="px-4 py-3 font-semibold whitespace-nowrap">客户端 / 设备标识</th>
              <th scope="col" class="px-4 py-3 font-semibold whitespace-nowrap">最近通信时间</th>
              <th scope="col" class="px-4 py-3 font-semibold whitespace-nowrap">Token 摘要</th>
              <th scope="col" class="px-6 py-3 font-semibold whitespace-nowrap text-right">操作</th>
            </tr>
          </thead>
          <tbody class="divide-y divide-slate-800/70">
            <tr v-if="tokens.length === 0">
              <td colspan="5" class="px-6 py-8 text-center text-xs text-slate-500">
                暂无已登录的客户端设备
              </td>
            </tr>
            <tr v-for="t in tokens" :key="t.token" class="hover:bg-slate-900/40 transition-colors tabular-nums">
              <th scope="row" class="px-6 py-3 font-medium text-white whitespace-nowrap">{{ t.username }}</th>
              <td class="px-4 py-3 text-xs text-slate-300 font-mono whitespace-nowrap">{{ t.client_name }}</td>
              <td class="px-4 py-3 text-xs text-slate-400 whitespace-nowrap font-mono">
                {{ new Date(t.last_used_at).toLocaleString() }}
              </td>
              <td class="px-4 py-3 font-mono text-xs text-slate-500 whitespace-nowrap">
                {{ t.token.slice(0, 12) }}…
              </td>
              <td class="px-6 py-3 text-right whitespace-nowrap">
                <button
                  type="button"
                  @click="handleRevokeToken(t)"
                  :aria-label="`注销设备 ${t.username} (${t.client_name})`"
                  class="cursor-pointer whitespace-nowrap px-2.5 py-1 rounded-lg bg-rose-950/60 hover:bg-rose-900 text-xs text-rose-300 border border-rose-800/50 transition-colors"
                >
                  注销设备
                </button>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>

    <!-- 用户弹窗 -->
    <div
      v-if="modal.visible"
      role="dialog"
      aria-modal="true"
      aria-labelledby="user-modal-title"
      class="fixed inset-0 bg-black/75 backdrop-blur-xs flex items-center justify-center z-50 p-4"
      @click.self="modal.visible = false"
    >
      <div class="card p-6 rounded-2xl max-w-md w-full space-y-4">
        <div class="flex items-center justify-between border-b border-slate-800 pb-3">
          <h3 id="user-modal-title" class="text-base font-bold text-white flex items-center gap-2">
            <UserPlus class="w-4 h-4 text-emerald-400" aria-hidden="true" />
            <span>{{ modal.id ? '编辑用户账号与分类权限' : '创建家庭成员账号' }}</span>
          </h3>
          <button
            type="button"
            aria-label="关闭用户弹窗"
            @click="modal.visible = false"
            class="cursor-pointer text-slate-400 hover:text-white p-1 rounded-lg hover:bg-slate-800 transition-colors"
          >
            <X class="w-4 h-4" aria-hidden="true" />
          </button>
        </div>

        <div class="space-y-3.5 text-xs">
          <div>
            <label for="user-username-input" class="block text-slate-300 mb-1.5">登录用户名</label>
            <input
              id="user-username-input"
              v-model="modal.username"
              type="text"
              autocomplete="username"
              placeholder="例如：kids 或 family"
              class="w-full bg-slate-950 border border-slate-700 focus:border-emerald-500 rounded-xl px-3.5 py-2 text-sm"
            />
          </div>
          <div>
            <label for="user-password-input" class="block text-slate-300 mb-1.5">
              登录密码（留空表示不修改密码或允许内网免密登录）
            </label>
            <input
              id="user-password-input"
              v-model="modal.password"
              type="password"
              autocomplete="new-password"
              placeholder="输入新密码…"
              class="w-full bg-slate-950 border border-slate-700 focus:border-emerald-500 rounded-xl px-3.5 py-2 text-sm"
            />
          </div>
          <div class="flex items-center gap-2 pt-1">
            <input
              id="user-is-admin-cb"
              v-model="modal.isAdmin"
              type="checkbox"
              class="rounded bg-slate-900 border-slate-700"
            />
            <label for="user-is-admin-cb" class="text-xs text-slate-200 font-medium cursor-pointer">
              授予系统管理员权限（拥有全库访问权）
            </label>
          </div>
          <fieldset class="space-y-1.5">
            <legend class="block text-slate-300">
              允许访问的媒体库分类（若勾选特定分类，将取消「全部不限」）
            </legend>
            <div class="grid grid-cols-2 gap-2 bg-slate-950/80 p-3.5 rounded-xl border border-slate-800 text-xs">
              <label
                v-for="opt in categoryOptions"
                :key="opt.value"
                class="flex items-center gap-2 cursor-pointer"
                :class="opt.wide ? 'col-span-2' : ''"
              >
                <input
                  type="checkbox"
                  :value="opt.value"
                  v-model="modal.allowedCategories"
                />
                <component :is="opt.icon" class="w-3.5 h-3.5 text-slate-400 shrink-0" aria-hidden="true" />
                <span class="text-slate-200">{{ opt.label }}</span>
              </label>
            </div>
          </fieldset>
        </div>

        <div class="flex justify-end gap-2.5 pt-3 border-t border-slate-800">
          <button
            type="button"
            @click="modal.visible = false"
            class="cursor-pointer bg-slate-800 hover:bg-slate-700 px-4 py-2 rounded-xl text-xs font-medium transition-colors"
          >
            取消
          </button>
          <button
            type="button"
            @click="submitUser"
            class="cursor-pointer bg-emerald-600 hover:bg-emerald-500 px-5 py-2 rounded-xl text-xs font-semibold text-white transition-colors"
          >
            保存配置
          </button>
        </div>
      </div>
    </div>
  </section>
</template>

<script setup>
import { ref, reactive, onMounted } from 'vue'
import {
  Activity,
  RefreshCw,
  FastForward,
  Play,
  Users,
  UserPlus,
  ShieldCheck,
  User,
  Lock,
  Unlock,
  Pencil,
  Trash2,
  KeyRound,
  X,
  LayoutGrid,
  Film,
  Tv,
  Sparkles,
  Globe,
  Mic
} from 'lucide-vue-next'
import { adminApi } from '../api/admin'
import { useToast, useConfirm } from '../composables/useToast'

const toast = useToast()
const confirmModal = useConfirm()
const users = ref([])
const tokens = ref([])
const activeSessions = ref([])

const categoryOptions = [
  { value: '*', label: '全部媒体库（不限）', icon: LayoutGrid },
  { value: '电影', label: '电影', icon: Film },
  { value: '电视剧', label: '电视剧', icon: Tv },
  { value: '动漫', label: '动漫', icon: Sparkles },
  { value: '纪录片', label: '纪录片', icon: Globe },
  { value: '综艺', label: '综艺', icon: Mic },
  { value: 'vlib_*', label: '允许访问所有智能视图', icon: Sparkles, wide: true }
]

const modal = reactive({
  visible: false,
  id: null,
  username: '',
  password: '',
  isAdmin: false,
  allowedCategories: ['*']
})

async function loadAll() {
  try {
    const data = await adminApi.listUsersAndSessions()
    users.value = data.users || []
    tokens.value = data.tokens || []
    activeSessions.value = data.active_sessions || []
  } catch (err) {
    toast.error('加载用户与会话失败：' + err.message)
  }
}

function openModal(u = null) {
  modal.id = u ? u.id : null
  modal.username = u ? u.username : ''
  modal.password = ''
  modal.isAdmin = u ? u.is_admin : false
  modal.allowedCategories = u ? [...(u.allowed_categories || ['*'])] : ['*']
  modal.visible = true
}

async function submitUser() {
  if (!modal.username.trim()) {
    return toast.warn('用户名不能为空')
  }
  try {
    const res = await adminApi.saveUser({
      id: modal.id,
      username: modal.username.trim(),
      password: modal.password !== '' ? modal.password : null,
      is_admin: modal.isAdmin,
      allowed_categories: modal.allowedCategories.length ? modal.allowedCategories : ['*']
    })
    toast.success(res.message)
    modal.visible = false
    await loadAll()
  } catch (err) {
    toast.error('保存账号失败：' + err.message)
  }
}

async function handleDeleteUser(u) {
  const ok = await confirmModal({
    title: '删除用户账号',
    message: `确认删除用户「${u.username}」及其关联的播放进度与收藏数据吗？`,
    confirmText: '确认删除',
    danger: true
  })
  if (!ok) return
  try {
    const res = await adminApi.deleteUser(u.id)
    toast.info(res.message)
    await loadAll()
  } catch (err) {
    toast.error('删除账号失败：' + err.message)
  }
}

async function handleRevokeToken(t) {
  const ok = await confirmModal({
    title: '注销客户端设备',
    message: `确认注销账号「${t.username}」在「${t.client_name}」上的登录状态吗？该设备需重新登录才能访问媒体库。`,
    confirmText: '确认注销',
    danger: true
  })
  if (!ok) return
  try {
    const res = await adminApi.revokeToken(t.token)
    toast.info(res.message)
    await loadAll()
  } catch (err) {
    toast.error('注销设备失败：' + err.message)
  }
}

onMounted(loadAll)
</script>
