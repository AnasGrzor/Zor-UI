import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'

test('uninstall.ps1 stops zorui.exe before deleting its folder', () => {
  const src = readFileSync(new URL('../uninstall.ps1', import.meta.url), 'utf8')
  assert.match(src, /Get-Process[^\n]*\bzorui\b[^\n]*Stop-Process/)
})

test('install.ps1 starts GlazeWM with a logon task and uninstall.ps1 removes it', () => {
  const inst = readFileSync(new URL('../install.ps1', import.meta.url), 'utf8')
  const un = readFileSync(new URL('../uninstall.ps1', import.meta.url), 'utf8')
  assert.match(inst, /Register-ScheduledTask -TaskName 'GlazeWM'/)
  assert.match(inst, /-AtLogOn/)
  assert.match(un, /Unregister-ScheduledTask -TaskName 'GlazeWM'/)
})

test('GlazeWM config template does not start the retired zebar', () => {
  const cfg = readFileSync(new URL('../glazewm/config.template.yaml', import.meta.url), 'utf8')
  assert.doesNotMatch(cfg, /shell-exec zebar/)
})
