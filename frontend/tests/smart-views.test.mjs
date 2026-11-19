import test from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import vm from 'node:vm'
import { transformSync } from 'esbuild'
import { parse } from '@vue/compiler-sfc'
import { ref, reactive, computed, watch } from 'vue'
import { validateSmartRule } from '../src/api/smartViewRules.js'
import { useVirtualLibraries } from '../src/composables/useVirtualLibraries.js'
import { coverImageUrl, dynamicCoverLabel, isCustomCover, isDynamicCover, ruleBadges } from '../src/utils/smartViewLabels.js'

function editor(api = {}) {
  const source = fs.readFileSync(new URL('../src/components/VirtualLibrariesTab.vue', import.meta.url),'utf8')
  const { descriptor } = parse(source)
  const script = descriptor.scriptSetup.content.replace(/^import[\s\S]*?from\s+['"][^'"]+['"]\s*$/mg,'')
  const stubToast = {warn:()=>{},success:()=>{},error:()=>{},info:()=>{}}
  const stubConfirm = async()=>true
  const stubApi = {listVirtualLibraries:async()=>({libraries:[]}),...api}
  const ctx = vm.createContext({
    ref, reactive, computed, watch, onMounted:()=>{}, onUnmounted:()=>{}, defineEmits:()=>()=>{},
    coverImageUrl, dynamicCoverLabel, isCustomCover, isDynamicCover, ruleBadges,
    useVirtualLibraries: (deps = {}) => useVirtualLibraries({
      api: stubApi,
      toast: stubToast,
      confirmModal: stubConfirm,
      validateRule: validateSmartRule,
      ...deps
    })
  })
  vm.runInContext(transformSync(script,{loader:'ts'}).code + '\nglobalThis.editor = {modal,openModal,submitModal,formError,saving,previewRules,previewCount,ruleOptions,optionsLoaded,needsMetadataBackfill,loadRuleOptions,backfillMetadata,importModal,openImportModal,handleImportBytes,confirmImportApply,handleDownloadBundle};',ctx)
  return ctx.editor
}

test('选项未加载完成或总数为 0 时不提示需补齐扩展信息，存在未补齐时才标记 needsMetadataBackfill', async()=>{
  let releaseOptions
  const e = editor({
    smartViewOptions: () => new Promise(r => { releaseOptions = r })
  })
  assert.equal(e.optionsLoaded.value, false)
  assert.equal(e.needsMetadataBackfill.value, false)

  const loadingPromise = e.loadRuleOptions()
  assert.equal(e.optionsLoaded.value, false)
  assert.equal(e.needsMetadataBackfill.value, false)

  releaseOptions({ networks: [], companies: [], collections: [], metadata: { total: 0, enriched: 0, queued: 0, failed: 0 } })
  await loadingPromise
  assert.equal(e.optionsLoaded.value, true)
  assert.equal(e.needsMetadataBackfill.value, false)

  const e2 = editor({
    smartViewOptions: async () => ({ networks: [], companies: [], collections: [], metadata: { total: 10, enriched: 6, queued: 0, failed: 0 } })
  })
  await e2.loadRuleOptions()
  assert.equal(e2.optionsLoaded.value, true)
  assert.equal(e2.needsMetadataBackfill.value, true)
})

test('智能视图列表页不再包含顶部 TMDB 扩展信息补齐进度条卡片', () => {
  const source = fs.readFileSync(new URL('../src/components/VirtualLibrariesTab.vue', import.meta.url), 'utf8')
  assert.equal(source.includes('TMDB 扩展信息已补齐'), false)
  assert.equal(source.includes('aria-label="TMDB 扩展信息补齐进度"'), false)
})

test('评分、倒置年份和小数集数不能保存', async()=>{
  for(const rules of [{min_rating:11},{year_min:2026,year_max:2020},{episodes_max:1.5}]) {
    let calls=0
    const e=editor({saveVirtualLibrary:async()=>{calls++;return {message:'ok'}}})
    e.openModal(); e.modal.name='规则校验'; e.modal.rules=rules
    await e.submitModal()
    assert.equal(calls,0);assert.ok(e.formError.value)
  }
})
test('响应前重复点击只提交一次',async()=>{
  let calls=0,release
  const e=editor({saveVirtualLibrary:async()=>{calls++;await new Promise(r=>{release=r});return {message:'ok'}}})
  e.openModal();e.modal.name='新视图'
  const first=e.submitModal();await e.submitModal()
  assert.equal(calls,1);assert.equal(e.saving.value,true)
  release();await first;assert.equal(e.saving.value,false)
})
test('保存失败保留弹窗与规则，并允许重试',async()=>{
  let calls=0
  const e=editor({saveVirtualLibrary:async()=>{calls++;throw new Error('请求失败')}})
  e.openModal();e.modal.name='保留规则';e.modal.rules={min_votes:1000}
  await e.submitModal();assert.equal(e.modal.visible,true);assert.equal(e.modal.rules.min_votes,1000)
  await e.submitModal();assert.equal(calls,2)
})
test('编辑往返保留新字段和嵌套条件组',async()=>{
  let saved
  const rules={countries:['中国大陆'],network_ids:[2552],has_collection:false,any:[{languages:['ja'],min_votes:1000}]}
  const e=editor({saveVirtualLibrary:async p=>{saved=JSON.parse(JSON.stringify(p));return {message:'ok'}}})
  e.openModal({id:1,name:'已有视图',icon_type:'tvshows',sort_order:10,is_enabled:true,filter_rules:rules})
  await e.submitModal();assert.deepEqual(saved.filter_rules,rules)
})
test('预览响应过期时不展示旧数量',async()=>{
  let release
  const e=editor({previewSmartView:()=>new Promise(r=>{release=r})})
  e.openModal();e.modal.rules={min_votes:100}
  const p=e.previewRules();e.modal.rules.min_votes=1000;release({matched_count:8});await p
  assert.equal(e.previewCount.value,null)
})
test('合法组合通过校验',()=>{
  assert.doesNotThrow(()=>validateSmartRule({min_rating:8,max_rating:10,min_votes:1000,any:[{runtime_max:90},{statuses:['Ended'],episodes_max:12}]}))
})

test('移除补充 9 个预设视图按钮，改用固定文案「下载配置与 AI 模板」和「导入视图配置」', () => {
  const source = fs.readFileSync(new URL('../src/components/VirtualLibrariesTab.vue', import.meta.url), 'utf8')
  assert.equal(source.includes('补充 9 个预设视图'), false)
  assert.equal(source.includes('handleSeedPresets'), false)
  assert.equal(source.includes('下载配置与 AI 模板'), true)
  assert.equal(source.includes('导入视图配置'), true)
})

test('自定义静态封面标识识别与 URL 解析', () => {
  assert.equal(isCustomCover('custom_0123456789abcdef'), true)
  assert.equal(isCustomCover('vlib_top_rated'), false)
  assert.equal(isCustomCover('collage_cards'), false)
  assert.equal(
    coverImageUrl('lib_vlib_1', 'custom_0123456789abcdef', 'vlib_default_tv', 123),
    '/covers/custom_0123456789abcdef.jpg?_t=123'
  )
})

test('导入视图配置两步预览与确认应用流程（含移除旧视图开关）', async () => {
  let previewReceivedBytes = null
  let appliedArgs = null
  let reloadCount = 0
  const e = editor({
    listVirtualLibraries: async () => {
      reloadCount++
      return { libraries: [] }
    },
    previewImportVirtualLibraries: async (bytes) => {
      previewReceivedBytes = bytes
      return {
        total_count: 2,
        create_count: 1,
        update_count: 1,
        custom_cover_count: 1,
        unmatched_existing_count: 3,
        items: [
          { name: '万人高分电影榜', action: 'update', icon_type: 'movies', sort_order: 10, is_enabled: true, cover_type: 'dynamic', cover_label: '立体叠放卡片', filter_rules: { media_type: 'movie', min_rating: 8 } },
          { name: '已完结全集爽看', action: 'create', icon_type: 'tvshows', sort_order: 20, is_enabled: true, cover_type: 'custom', cover_label: '自定义封面', filter_rules: { media_type: 'tv', local_complete: 'all' } }
        ]
      }
    },
    applyImportVirtualLibraries: async (bytes, pruneMissing) => {
      appliedArgs = { bytes, pruneMissing }
      return { status: 'ok', created: 1, updated: 1, deleted: 3, message: '已应用视图配置' }
    }
  })

  e.openImportModal()
  assert.equal(e.importModal.visible, true)
  assert.equal(e.importModal.pruneMissing, false)

  const sampleBytes = new Uint8Array([123, 125])
  await e.handleImportBytes('views.zip', sampleBytes)
  assert.equal(e.importModal.fileName, 'views.zip')
  assert.deepEqual(previewReceivedBytes, sampleBytes)
  assert.equal(e.importModal.preview.total_count, 2)
  assert.equal(e.importModal.preview.unmatched_existing_count, 3)

  e.importModal.pruneMissing = true
  await e.confirmImportApply()
  assert.deepEqual(appliedArgs, { bytes: sampleBytes, pruneMissing: true })
  assert.equal(e.importModal.visible, false)
  assert.equal(reloadCount, 1)
})
