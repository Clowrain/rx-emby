import test from 'node:test'
import assert from 'node:assert/strict'
import { formatBuildTime, getBuildInfo } from '../src/utils/buildInfo.js'

test('formatBuildTime formats valid ISO timestamps and rejects invalid ones', () => {
  assert.equal(formatBuildTime(''), '')
  assert.equal(formatBuildTime('not-a-date'), '')
  assert.match(formatBuildTime('2026-11-14T14:12:00.000Z'), /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}$/)
})

test('getBuildInfo normalizes version, dirty state, and diagnostic text', () => {
  const info = getBuildInfo({
    version: '2.0.0',
    gitSha: 'f7607b5',
    gitBranch: 'main',
    gitDirty: true,
    buildTime: '2026-11-14T14:12:00.000Z'
  })

  assert.equal(info.version, 'v2.0.0')
  assert.equal(info.commitLabel, 'f7607b5-dirty')
  assert.ok(info.diagnosticText.startsWith('RX-Emby v2.0.0 · f7607b5-dirty · main · built '))
})

test('getBuildInfo handles missing fields gracefully', () => {
  const info = getBuildInfo({})
  assert.equal(info.version, 'v0.0.0')
  assert.equal(info.commitLabel, 'unknown')
  assert.equal(info.diagnosticText, 'RX-Emby v0.0.0 · unknown')
})
