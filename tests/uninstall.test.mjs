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

test('the logon task fallback starts glazewm through cmd start (uiAccess exes fail with 740 otherwise) after a delay', () => {
  const inst = readFileSync(new URL('../install.ps1', import.meta.url), 'utf8')
  assert.match(inst, /New-ScheduledTaskAction -Execute 'cmd\.exe' -Argument "\/c start/)
  assert.match(inst, /\$trg\.Delay = 'PT\d+S'/)
})

test('install.ps1 installs the ZorShell boot service as admin and uninstall.ps1 removes it', () => {
  const inst = readFileSync(new URL('../install.ps1', import.meta.url), 'utf8')
  const un = readFileSync(new URL('../uninstall.ps1', import.meta.url), 'utf8')
  assert.match(inst, /sc\.exe create ZorShell/)
  assert.match(inst, /start= auto/)
  assert.match(inst, /sc\.exe failure ZorShell/)
  assert.match(inst, /IsInRole/)
  // svc.json lists what the service starts with an admin token: only SYSTEM and Administrators may write it
  assert.match(inst, /icacls \$svcData \/inheritance:r[^\n]*Administrators:\(OI\)\(CI\)F[^\n]*Users:\(OI\)\(CI\)RX/)
  assert.match(inst, /\$svcDir = Join-Path \$env:ProgramFiles/)
  // the Settings switch writes startup.json, so Users may write that one file; a re-install keeps its value
  assert.match(inst, /Test-Path \$flag/)
  assert.match(inst, /icacls \$flag \/grant:r 'Users:M'/)
  assert.match(un, /Stop-Service ZorShell/)
  assert.match(un, /sc\.exe delete ZorShell/)
})

test('uninstall.ps1 removes the ZorShell service before it stops zorui', () => {
  const un = readFileSync(new URL('../uninstall.ps1', import.meta.url), 'utf8')
  assert.ok(un.indexOf('sc.exe delete ZorShell') < un.indexOf('Get-Process zorui'))
})

test('install.ps1 starts ZorShell at every logon (Fast Startup does not start services) and uninstall.ps1 removes the task first', () => {
  const inst = readFileSync(new URL('../install.ps1', import.meta.url), 'utf8')
  const un = readFileSync(new URL('../uninstall.ps1', import.meta.url), 'utf8')
  assert.match(inst, /New-ScheduledTaskAction -Execute 'sc\.exe' -Argument 'start ZorShell'/)
  assert.match(inst, /Register-ScheduledTask -TaskName 'ZorShellLogon'/)
  assert.match(un, /Unregister-ScheduledTask -TaskName 'ZorShellLogon'/)
  assert.ok(un.indexOf("'ZorShellLogon'") < un.indexOf('sc.exe delete ZorShell'))
})

test('install.ps1 cleans up a leftover Zebar and moves settings out of its old folder first', () => {
  const inst = readFileSync(new URL('../install.ps1', import.meta.url), 'utf8')
  assert.match(inst, /\$Pack = Join-Path \$Glzr 'zor-ui'/)
  assert.match(inst, /Get-Process zebar[^\n]*Stop-Process/)
  assert.match(inst, /winget uninstall --id glzr-io\.zebar/)
  assert.match(inst, /Get-ChildItem \(\[Environment\]::GetFolderPath\('Startup'\)\) -Filter '\*zebar\*'/)
  assert.match(inst, /CurrentVersion\\Run/)
  assert.match(inst, /Where-Object \{ \$_\.TaskName -match 'zebar' \} \| Unregister-ScheduledTask/)
  assert.ok(inst.indexOf('Move-Item $op $Pack') < inst.indexOf("Move-Item $zebarDir"))
})

test('uninstall.ps1 removes the settings folder and leaves Zebar alone', () => {
  const un = readFileSync(new URL('../uninstall.ps1', import.meta.url), 'utf8')
  assert.match(un, /Join-Path \$Glzr 'zor-ui'/)
  assert.doesNotMatch(un, /zebar/i)
})

test('GlazeWM config template does not start the leftover zebar', () => {
  const cfg = readFileSync(new URL('../glazewm/config.template.yaml', import.meta.url), 'utf8')
  assert.doesNotMatch(cfg, /shell-exec zebar/)
})

test('GlazeWM config template ignores the Valorant and Riot Client windows', () => {
  const cfg = readFileSync(new URL('../glazewm/config.template.yaml', import.meta.url), 'utf8')
  const rule = cfg.match(/window_process: \{ regex: '(\^\(VALORANT[^']*)' \}/)
  assert.ok(rule, 'rule present')
  const re = new RegExp(rule[1])
  for (const p of ['VALORANT-Win64-Shipping', 'VALORANT', 'RiotClientServices', 'RiotClientUx', 'Riot Client', 'vgtray']) {
    assert.match(p, re)
  }
  assert.doesNotMatch('chrome', re)
})
