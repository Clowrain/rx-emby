<template>
  <div class="card p-6 rounded-2xl space-y-5">
    <div class="flex flex-wrap items-center justify-between gap-3 border-b border-slate-800 pb-4">
      <div>
        <h2 class="text-base font-bold text-white flex items-center gap-2">
          <component :is="icon" v-if="icon" class="w-4 h-4 text-emerald-400" aria-hidden="true" />
          <span>{{ title }}</span>
        </h2>
        <p v-if="description" class="text-xs text-slate-400 mt-1">{{ description }}</p>
      </div>
      <button
        type="button"
        @click="handleSave"
        :disabled="saving || loading"
        class="cursor-pointer bg-emerald-600 hover:bg-emerald-500 disabled:opacity-50 text-white px-5 py-2 rounded-xl text-xs font-semibold transition-colors shadow-lg shadow-emerald-950/60 flex items-center gap-1.5"
      >
        <Check class="w-3.5 h-3.5" aria-hidden="true" />
        <span>{{ saving ? '正在保存…' : '保存并立即生效' }}</span>
      </button>
    </div>

    <div v-if="loading" class="text-center py-8 text-slate-400 text-xs">正在加载配置…</div>

    <div v-else class="grid grid-cols-1 md:grid-cols-2 gap-4">
      <div
        v-for="c in filteredConfigs"
        :key="c.config_key"
        class="bg-slate-950/70 p-4 rounded-xl border border-slate-800/90 flex flex-col justify-between space-y-3"
      >
        <div class="space-y-1">
          <div class="flex items-center justify-between gap-2">
            <label :for="`cfg-${c.config_key}`" class="font-semibold text-sm text-slate-100">
              {{ friendlyLabel(c.config_key) }}
            </label>
            <span class="text-[11px] font-mono text-slate-500">{{ c.config_key }}</span>
          </div>
          <p :id="`desc-${c.config_key}`" class="text-xs text-slate-400 leading-relaxed">
            {{ friendlyDesc(c) }}
          </p>
        </div>

        <div v-if="isBooleanConfig(c)" class="flex items-center justify-between pt-1">
          <span class="text-xs font-mono" :class="c.config_value === 'true' ? 'text-emerald-400' : 'text-slate-500'">
            {{ c.config_value === 'true' ? '已启用' : '已关闭' }}
          </span>
          <button
            :id="`cfg-${c.config_key}`"
            type="button"
            role="switch"
            :aria-checked="c.config_value === 'true'"
            :aria-describedby="`desc-${c.config_key}`"
            @click="c.config_value = c.config_value === 'true' ? 'false' : 'true'"
            class="cursor-pointer w-12 h-6 rounded-full p-0.5 transition-colors duration-200 flex items-center"
            :class="c.config_value === 'true' ? 'bg-emerald-600 justify-end' : 'bg-slate-800 justify-start'"
          >
            <span class="w-5 h-5 rounded-full bg-white shadow"></span>
          </button>
        </div>

        <div v-else-if="c.value_type === 'secret'" class="relative">
          <input
            :id="`cfg-${c.config_key}`"
            v-model="c.config_value"
            :type="revealedKeys[c.config_key] ? 'text' : 'password'"
            :aria-describedby="`desc-${c.config_key}`"
            autocomplete="off"
            class="w-full bg-slate-900 border border-slate-700 focus:border-emerald-500 rounded-xl pl-3.5 pr-16 py-2 text-xs font-mono text-slate-100"
          />
          <button
            type="button"
            :aria-label="`${revealedKeys[c.config_key] ? '隐藏' : '显示'} ${friendlyLabel(c.config_key)}`"
            @click="revealedKeys[c.config_key] = !revealedKeys[c.config_key]"
            class="cursor-pointer absolute right-2.5 top-1/2 -translate-y-1/2 text-[11px] text-slate-400 hover:text-slate-200 px-1.5 py-0.5 rounded flex items-center gap-1"
          >
            <EyeOff v-if="revealedKeys[c.config_key]" class="w-3.5 h-3.5" aria-hidden="true" />
            <Eye v-else class="w-3.5 h-3.5" aria-hidden="true" />
            <span>{{ revealedKeys[c.config_key] ? '隐藏' : '显示' }}</span>
          </button>
        </div>

        <input
          v-else
          :id="`cfg-${c.config_key}`"
          v-model="c.config_value"
          type="text"
          :aria-describedby="`desc-${c.config_key}`"
          class="w-full bg-slate-900 border border-slate-700 focus:border-emerald-500 rounded-xl px-3.5 py-2 text-xs font-mono text-slate-100"
        />
      </div>
    </div>
  </div>
</template>

<script setup>
import { ref, reactive, computed, onMounted } from 'vue'
import { Check, Eye, EyeOff } from 'lucide-vue-next'
import { adminApi } from '../api/admin'
import { useToast } from '../composables/useToast'

const props = defineProps({
  prefix: { type: String, required: true },
  title: { type: String, required: true },
  description: { type: String, default: '' },
  icon: { type: [Object, Function], default: null }
})

const emit = defineEmits(['saved'])
const toast = useToast()
const loading = ref(false)
const saving = ref(false)
const configs = ref([])
const revealedKeys = reactive({})

const labelMap = {
  'alist.base_url': '小雅 / AList 服务地址',
  'alist.username': 'AList 登录用户名',
  'alist.password': 'AList 登录密码',
  'alist.otp_secret': '两步验证 OTP 密钥（可选）',
  'alist.path_prefix': '挂载子目录前缀映射',
  'alist.direct_url_ttl_sec': '302 网盘直链缓存有效期（秒）',
  'tmdb.api_key': 'TMDB API 密钥',
  'tmdb.proxy_url': 'HTTP / SOCKS5 代理地址',
  'tmdb.api_base_url': 'TMDB API 基础地址',
  'tmdb.image_base_url': 'TMDB 图片源基础地址',
  'tmdb.rate_limit_per_sec': 'TMDB 每秒请求限流阈值（req/s）',
  'scraper.auto_background': '后台全库自动刮削总开关',
  'sync.strm_txt_source': 'strm.txt / strm.zip 索引路径或 URL',
  'sync.sync_interval_sec': '增量同步定时间隔（秒）',
  'sync.parse_concurrency': '索引解析并发线程数',
  'sync.db_batch_size': '数据库单批写入行数',
  'sync.include_prefixes': '同步目录白名单前缀（逗号分隔）',
  'sync.exclude_keywords': '同步排除黑名单关键词（逗号分隔）',
  'sync.max_lines_limit': '单次最大解析行数限制（0 为不限）',
  'image_cache.disk_cache_enabled': '海报与剧照本地磁盘缓存',
  'image_cache.cache_dir': '本地图片缓存目录',
  'image_cache.max_size_mb': '图片缓存最大磁盘配额（MB）',
  'image_cache.evict_high_watermark': '缓存清理触发高水位比例',
  'image_cache.evict_low_watermark': '缓存清理目标低水位比例'
}

const descMap = {
  'alist.base_url': '填写小雅或 AList 服务的访问地址，例如 http://192.168.1.10:5678',
  'alist.username': '用于自动获取 AList 访问令牌的登录账号（若开启匿名访问可留空）',
  'alist.password': '对应 AList 账号的登录密码（已加密掩码展示）',
  'alist.otp_secret': '若 AList 账号启用了两步验证，填写 Base32 TOTP 密钥',
  'alist.path_prefix': '若小雅挂载在 AList 子目录下（如 /xiaoya），请填写该前缀；根目录留空即可',
  'alist.direct_url_ttl_sec': '网盘 302 直链在内存中的复用时间，建议设为 900~1800 秒以减少网盘请求',
  'tmdb.api_key': '用于抓取影视海报、简介、评分与分集信息的 TMDB v3 API Key',
  'tmdb.proxy_url': '访问 TMDB 时的网络代理（例如 http://127.0.0.1:7890），直连可留空',
  'tmdb.api_base_url': '默认 https://api.themoviedb.org/3，可替换为自建反代地址',
  'tmdb.image_base_url': '默认 https://image.tmdb.org/t/p，用于回源下载海报与剧照',
  'tmdb.rate_limit_per_sec': '限制每秒向 TMDB 发起的请求次数，建议保持在 8~20 之间以防触发限流',
  'scraper.auto_background': '开启后将在后台持续刮削待处理影视；关闭后仅在点开条目或手动点击时按需刮削',
  'sync.strm_txt_source': '支持本地 .txt/.zip 路径或远程 HTTP(S) 压缩包地址（如 GitHub strm.zip）',
  'sync.sync_interval_sec': '后台自动检查并同步 strm.txt 变更的时间间隔（例如 3600 表示每小时一次）',
  'sync.parse_concurrency': '解析路径与计算哈希的并发工作线程数，建议与 CPU 核心数相当',
  'sync.db_batch_size': '每次批量写入数据库的记录条数，默认 2000~5000 即可兼顾速度与内存占用',
  'sync.include_prefixes': '仅同步匹配指定前缀的目录（多个用逗号分隔），留空则同步全部目录',
  'sync.exclude_keywords': '路径中包含这些关键词的条目将被自动跳过（多个用逗号分隔）',
  'sync.max_lines_limit': '设为 0 表示全量读取；调试时可设为较小数值（如 5000）以加快测试速度',
  'image_cache.disk_cache_enabled': '开启后将拉取的 TMDB 图片落盘缓存并启用 LRU 淘汰；关闭后不写入本地磁盘，由后台实时网络代理直接返回图片流',
  'image_cache.cache_dir': '海报与剧照在本地磁盘上的存储目录路径',
  'image_cache.max_size_mb': '本地图片缓存允许占用的最大磁盘空间（MB）',
  'image_cache.evict_high_watermark': '当缓存占用超过该比例（如 0.90 即 90%）时，自动触发过期清理',
  'image_cache.evict_low_watermark': '触发清理后将缓存空间回落至该安全比例（如 0.75 即 75%）'
}

const filteredConfigs = computed(() => {
  const prefixes = props.prefix.split('|')
  return configs.value.filter(c => prefixes.some(p => c.config_key.startsWith(p)))
})

function friendlyLabel(key) {
  return labelMap[key] || key
}

function friendlyDesc(c) {
  return descMap[c.config_key] || c.description || '系统运行参数'
}

function isBooleanConfig(c) {
  return (
    c.value_type === 'bool' ||
    c.value_type === 'boolean' ||
    c.config_key === 'scraper.auto_background' ||
    c.config_value === 'true' ||
    c.config_value === 'false'
  )
}

async function loadConfigs() {
  if (configs.value.length === 0) {
    loading.value = true
  }
  try {
    const data = await adminApi.listConfigs()
    configs.value = data.configs || []
  } catch (err) {
    toast.error('加载配置失败：' + err.message)
  } finally {
    loading.value = false
  }
}

async function handleSave() {
  saving.value = true
  try {
    const updates = {}
    for (const c of filteredConfigs.value) {
      updates[c.config_key] = c.config_value
    }
    const res = await adminApi.updateConfigs(updates)
    toast.success(res.message || '设置已保存并立即生效')
    emit('saved')
    await loadConfigs()
  } catch (err) {
    toast.error('保存失败：' + err.message)
  } finally {
    saving.value = false
  }
}

onMounted(loadConfigs)
</script>
