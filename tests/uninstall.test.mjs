import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'

test('uninstall.ps1 stops zorui.exe before deleting its folder', () => {
  const src = readFileSync(new URL('../uninstall.ps1', import.meta.url), 'utf8')
  assert.match(src, /Get-Process[^\n]*\bzorui\b[^\n]*Stop-Process/)
})
