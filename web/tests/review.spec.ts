import { expect, test, type Page, type Route } from '@playwright/test'

function pdfDocument(pages = 4) {
  const objects = [
    '<< /Type /Catalog /Pages 2 0 R >>',
    `<< /Type /Pages /Kids [${Array.from({ length: pages }, (_, i) => `${i + 3} 0 R`).join(' ')}] /Count ${pages} >>`,
    ...Array.from({ length: pages }, () => '<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> >>'),
  ]
  let pdf = '%PDF-1.4\n'
  const offsets = [0]
  for (const [i, object] of objects.entries()) {
    offsets.push(pdf.length)
    pdf += `${i + 1} 0 obj\n${object}\nendobj\n`
  }
  const xref = pdf.length
  pdf += `xref\n0 ${offsets.length}\n0000000000 65535 f \n`
  pdf += offsets.slice(1).map((offset) => `${String(offset).padStart(10, '0')} 00000 n \n`).join('')
  return pdf + `trailer\n<< /Size ${offsets.length} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`
}

async function mockVault(page: Page, initial: Record<string, string>) {
  const notes = new Map(Object.entries(initial).map(([path, content]) => [path, { path, content, mtime: '1' }]))
  let save: ((route: Route, path: string) => Promise<void>) | undefined
  let read: ((route: Route, path: string) => Promise<void>) | undefined
  await page.addInitScript(() => {
    class Events {
      onmessage: ((event: { data: string }) => void) | null = null
      constructor() {
        ;(window as any).vaultEvent = (path: string) => this.onmessage?.({ data: JSON.stringify({ path, op: 'write' }) })
      }
      close() {}
    }
    ;(window as any).EventSource = Events
  })
  await page.route('**/api/**', async (route) => {
    const req = route.request()
    const url = new URL(req.url())
    if (url.pathname === '/api/setup') return route.fulfill({ json: { configured: true } })
    if (url.pathname === '/api/tree') {
      return route.fulfill({ json: [...notes.keys()].map((path) => ({ name: path, path, dir: false })) })
    }
    if (url.pathname === '/api/backlinks') return route.fulfill({ json: [] })
    if (url.pathname.startsWith('/api/files/') && url.pathname.endsWith('.pdf')) {
      return route.fulfill({ body: pdfDocument(), contentType: 'application/pdf' })
    }
    const path = decodeURIComponent(url.pathname.slice('/api/notes/'.length))
    if (url.pathname.startsWith('/api/notes/')) {
      if (req.method() === 'GET') {
        if (read) return read(route, path)
        return route.fulfill({ json: notes.get(path) })
      }
      if (save) return save(route, path)
      const { content } = req.postDataJSON()
      const note = { path, content, mtime: String(Number(notes.get(path)?.mtime ?? 0) + 1) }
      notes.set(path, note)
      return route.fulfill({ json: note })
    }
    return route.fulfill({ status: 404, json: { error: 'not found' } })
  })
  return {
    notes,
    onSave(handler: typeof save) { save = handler },
    onRead(handler: typeof read) { read = handler },
  }
}

async function edit(page: Page, text: string) {
  await page.locator('.cm-content').click()
  await page.keyboard.press('i')
  await page.keyboard.insertText(text)
  await page.keyboard.press('Escape')
}

function deferred() {
  let resolve!: () => void
  const promise = new Promise<void>((r) => { resolve = r })
  return { promise, resolve }
}

test('an explicit first-page PDF link takes precedence over the remembered page', async ({ page }) => {
  await mockVault(page, { 'A.md': '[[book.pdf#page=1]]', 'book.pdf': '' })
  await page.addInitScript(() => localStorage.setItem('quirenote:page:book.pdf', '3'))
  await page.goto('/#note=A.md')
  await expect(page.locator('.editor .status')).toHaveText('saved')
  await page.getByRole('button', { name: 'Preview', exact: true }).click()
  await page.getByRole('link', { name: 'book.pdf · p.1', exact: true }).click()
  await expect(page.locator('.reader .toolbar .muted')).toHaveText('/ 4')
  await expect(page.getByRole('spinbutton', { name: 'Page', exact: true })).toHaveValue('1')
})

test('switching notes waits for an in-flight save and keeps each note separate', async ({ page }) => {
  const vault = await mockVault(page, { 'A.md': 'alpha', 'B.md': 'beta' })
  const started = deferred()
  const release = deferred()
  vault.onSave(async (route, path) => {
    if (path === 'A.md') {
      started.resolve()
      await release.promise
    }
    const { content } = route.request().postDataJSON()
    const note = { path, content, mtime: '2' }
    vault.notes.set(path, note)
    await route.fulfill({ json: note })
  })
  await page.goto('/#note=A.md')
  await expect(page.locator('.editor .status')).toHaveText('saved')
  await edit(page, 'A edit ')
  await page.keyboard.press('Control+s')
  await started.promise
  await page.getByTitle('B.md', { exact: true }).click()
  release.resolve()
  await expect(page.locator('.cm-content')).toHaveText('beta')
  await expect(page.locator('.editor .status')).toHaveText('saved')
  await edit(page, 'B edit ')
  await page.keyboard.press('Control+s')
  await expect.poll(() => vault.notes.get('B.md')?.content).toContain('B edit')
  expect(vault.notes.get('A.md')?.content).toContain('A edit')
  expect(vault.notes.get('A.md')?.content).not.toContain('beta')
})

test('a failed note load clears the previous buffer and retry reloads the selected note', async ({ page }) => {
  const vault = await mockVault(page, { 'A.md': 'alpha', 'B.md': 'beta' })
  await page.goto('/#note=A.md')
  await expect(page.locator('.editor .status')).toHaveText('saved')
  vault.onRead(async (route) => {
    await route.fulfill({ status: 503, json: { error: 'offline' } })
  })
  await page.getByTitle('B.md', { exact: true }).click()
  await expect(page.locator('.editor .status')).toHaveText('error')
  await expect(page.locator('.cm-content')).toHaveText('')
  await edit(page, 'must not edit the old note')
  await page.keyboard.press('Control+s')
  expect(vault.notes.get('A.md')?.content).toBe('alpha')
  vault.onRead(undefined)
  await page.getByRole('button', { name: 'try again', exact: true }).click()
  await expect(page.locator('.cm-content')).toHaveText('beta')
  await expect(page.locator('.editor .status')).toHaveText('saved')
})

test('a delayed external-change response cannot replace a different note', async ({ page }) => {
  const vault = await mockVault(page, { 'A.md': 'alpha', 'B.md': 'beta' })
  await page.goto('/#note=A.md')
  await expect(page.locator('.editor .status')).toHaveText('saved')
  const started = deferred()
  const release = deferred()
  vault.onRead(async (route, path) => {
    if (path === 'A.md') {
      started.resolve()
      await release.promise
      await route.fulfill({ json: { path, content: 'external alpha', mtime: '2' } })
    } else await route.fulfill({ json: vault.notes.get(path) })
  })
  await page.evaluate(() => (window as any).vaultEvent('A.md'))
  await started.promise
  await page.getByTitle('B.md', { exact: true }).click()
  await expect(page.locator('.cm-content')).toHaveText('beta')
  const response = page.waitForResponse('**/api/notes/A.md')
  release.resolve()
  await (await response).finished()
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))))
  await expect(page.locator('.cm-content')).toHaveText('beta')
})

for (const status of [409, 503]) {
  test(`a failed save (${status}) keeps unsaved text when switching notes or using Back`, async ({ page }) => {
    const vault = await mockVault(page, { 'A.md': 'alpha', 'B.md': 'beta' })
    await page.goto('/#note=B.md')
    await expect(page.locator('.editor .status')).toHaveText('saved')
    await page.getByTitle('A.md', { exact: true }).click()
    await expect(page.locator('.cm-content')).toHaveText('alpha')
    vault.onSave(async (route) => {
      await route.fulfill({ status, json: status === 409 ? { path: 'A.md', content: 'disk change', mtime: '2' } : { error: 'offline' } })
    })
    await edit(page, 'unsaved ')
    await page.getByTitle('B.md', { exact: true }).click()
    await expect(page.getByRole('status')).toContainText('before leaving')
    await expect(page.locator('.crumb')).toHaveText('A.md')
    await expect(page.locator('.cm-content')).toContainText('unsaved')
    await page.evaluate(() => history.back())
    await expect(page).toHaveURL(/note=A.md/)
    await expect(page.locator('.cm-content')).toContainText('unsaved')
    expect(vault.notes.get('A.md')?.content).toBe('alpha')
  })
}
