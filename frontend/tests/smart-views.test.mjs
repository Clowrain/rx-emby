import test from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import vm from 'node:vm'
import { transformSync } from 'esbuild'
import { parse } from '@vue/compiler-sfc'
import { ref, reactive, watch } from 'vue'
import { validateSmartRule } from '../src/api/smartViewRules.js'

function editor(api = {}) {
  const source = fs.readFileSync(new URL('../src/components/VirtualLibrariesTab.vue', import.meta.url),'utf8')
  const { descriptor } = parse(source)
  const script = descriptor.scriptSetup.content.replace(/^import .*$/mg,'')
  const ctx = vm.createContext({ref, reactive, watch, onMounted:()=>{},validateSmartRule,
    useToast:()=>({warn:()=>{},success:()=>{},error:()=>{},info:()=>{}}),useConfirm:()=>async()=>true,
    adminApi:{listVirtualLibraries:async()=>({libraries:[]}),...api}})
  vm.runInContext(transformSync(script,{loader:'ts'}).code + '\nglobalThis.editor = {modal,openModal,submitModal,formError,saving,previewRules,previewCount};',ctx)
  return ctx.editor
}

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
