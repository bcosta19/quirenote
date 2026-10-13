// Drives the REAL frontend (served by the backend under test) with the system Chromium:
// load the tree, open a note, edit it, let the autosave run, reload and check the text is back;
// then change the file on disk and check the editor picks it up through SSE.
//
//   node frontend.mjs --url http://127.0.0.1:9120 --vault /tmp/qbench/e2e-rs/vault [--shots DIR]
//
// Uses playwright-core from web/node_modules. Exit code 0 = all steps passed.
import { chromium } from '../../web/node_modules/playwright-core/index.mjs'
import fs from 'node:fs'
import path from 'node:path'

const arg = (n, d) => { const i = process.argv.indexOf('--' + n); return i > 0 ? process.argv[i + 1] : d }
const url = arg('url'), vault = arg('vault'), shots = arg('shots', '')
if (!url || !vault) { console.error('usage: --url URL --vault DIR [--shots DIR]'); process.exit(2) }
if (!vault.startsWith('/tmp/qbench/')) { console.error('refusing: vault must be under /tmp/qbench'); process.exit(2) }

const results = []
const step = async (name, fn) => {
  try { await fn(); results.push([name, true]); console.log('ok   ', name) }
  catch (e) { results.push([name, false]); console.log('FAIL ', name, '\n      ', String(e.message).split('\n')[0]) }
}
const until = async (what, fn, ms = 8000) => {
  const end = Date.now() + ms
  for (;;) { const v = await fn(); if (v) return v; if (Date.now() > end) throw new Error('timeout: ' + what); await new Promise(r => setTimeout(r, 100)) }
}

const browser = await chromium.launch({ executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH || '/usr/bin/chromium', headless: true, args: ['--no-sandbox'] })
const ctx = await browser.newContext({ viewport: { width: 1280, height: 800 } })
const page = await ctx.newPage()
const apiErrors = [], consoleErrors = []
page.on('response', r => { if (r.url().includes('/api/') && r.status() >= 500) apiErrors.push(r.status() + ' ' + r.url()) })
page.on('pageerror', e => consoleErrors.push(String(e)))
const shot = async n => { if (shots) { fs.mkdirSync(shots, { recursive: true }); await page.screenshot({ path: path.join(shots, n + '.png') }) } }

const note = 'Estudos/Compiladores.md'
const marker = 'EDITED-BY-PLAYWRIGHT-' + Date.now()

await step('page loads and the tree shows the vault', async () => {
  await page.goto(url)
  await page.locator('button.item[title="Welcome.md"]').waitFor({ timeout: 10000 })
  await shot('1-tree')
})
await step('folders expand and show nested notes', async () => {
  await page.locator('button.dir', { hasText: 'Estudos' }).first().click()
  await page.locator(`button.item[title="${note}"]`).waitFor({ timeout: 5000 })
})
await step('opening a note loads its content in the editor', async () => {
  await page.locator(`button.item[title="${note}"]`).click()
  await until('editor content', async () => (await page.locator('.cm-content').first().innerText()).includes('Compiladores'))
  await shot('2-note')
})
await step('typing then autosave writes the file on disk', async () => {
  await page.locator('.cm-content').first().click()
  // The editor is in Vim mode: Escape, G (last line), o (open a line below), type, Escape.
  await page.keyboard.press('Escape')
  await page.keyboard.type('Go' + marker)
  await page.keyboard.press('Escape')
  await until('file on disk', async () => fs.readFileSync(path.join(vault, note), 'utf8').includes(marker), 10000)
  await shot('3-saved')
})
await step('reload: the edit is still there', async () => {
  await page.reload()
  await page.locator('button.dir', { hasText: 'Estudos' }).first().waitFor()
  // the app reopens the last note (resume where you left off); open it explicitly if not
  const has = async () => (await page.locator('.cm-content').count()) > 0 && (await page.locator('.cm-content').first().innerText()).includes(marker)
  if (!(await has().catch(() => false))) {
    const dir = page.locator(`button.item[title="${note}"]`)
    if (!(await dir.count())) await page.locator('button.dir', { hasText: 'Estudos' }).first().click()
    await page.locator(`button.item[title="${note}"]`).click()
  }
  await until('edit visible after reload', has)
  await shot('4-reloaded')
})
await step('a change made on disk reaches the open editor (SSE)', async () => {
  const p = path.join(vault, note)
  const external = fs.readFileSync(p, 'utf8') + '\nEXTERNAL-EDIT-' + marker
  fs.writeFileSync(p, external)
  const later = new Date(Date.now() + 2000)
  fs.utimesSync(p, later, later)
  await until('external edit shown', async () => (await page.locator('.cm-content').first().innerText()).includes('EXTERNAL-EDIT-' + marker), 8000)
  await shot('5-external')
})
await step('a note created on disk by another program shows up in the tree (SSE)', async () => {
  // tmp + rename, like Neovim/Syncthing: a plain create+write would be coalesced into "write",
  // which the app does not treat as a tree change.
  fs.writeFileSync(path.join(vault, '.from-disk.tmp'), '# from disk\n')
  fs.renameSync(path.join(vault, '.from-disk.tmp'), path.join(vault, 'from-disk.md'))
  await until('tree refresh shows the new note', async () => (await page.locator('button.item[title="from-disk.md"]').count()) > 0, 8000)
})
await step('creating a note through the API from the page works (POST /api/notes)', async () => {
  const r = await page.evaluate(async () => (await fetch('/api/notes/ui-created.md', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ content: '# made in the browser\n' }) })).status)
  if (r !== 201) throw new Error('status ' + r)
  if (fs.readFileSync(path.join(vault, 'ui-created.md'), 'utf8') !== '# made in the browser\n') throw new Error('content on disk differs')
})
await step('no 5xx from the API and no uncaught page errors', async () => {
  if (apiErrors.length || consoleErrors.length) throw new Error(JSON.stringify({ apiErrors, consoleErrors }))
})

await browser.close()
const failed = results.filter(r => !r[1]).length
console.log(`\n${results.length - failed}/${results.length} steps passed`)
process.exit(failed ? 1 : 0)
