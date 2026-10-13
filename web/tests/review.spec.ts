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

/** The font actually in use by an element, with every fallback of the stack. */
function fontOf(page: Page, selector: string) {
  return page.evaluate((sel) => getComputedStyle(document.querySelector(sel)!).fontFamily, selector)
}

/** The text field of a role in the fonts dialog (the reset button next to it has a similar name). */
function fontField(page: Page, label: 'Interface' | 'Notes' | 'Code') {
  return page.getByRole('combobox', { name: new RegExp(`^${label}`) })
}

async function openFonts(page: Page) {
  await page.getByRole('button', { name: 'Fonts', exact: true }).click()
  await expect(page.getByRole('dialog', { name: 'Fonts' })).toBeVisible()
}

async function rememberLocalPdf(page: Page, pdf: string) {
  return page.evaluate(async (pdf) => {
    const root = await navigator.storage.getDirectory()
    const handle = await root.getFileHandle('local.pdf', { create: true })
    const writer = await handle.createWritable()
    await writer.write(pdf)
    await writer.close()
    const file = await handle.getFile()
    await new Promise<void>((resolve, reject) => {
      const open = indexedDB.open('quirenote', 1)
      open.onupgradeneeded = () => open.result.createObjectStore('files')
      open.onerror = () => reject(open.error)
      open.onsuccess = () => {
        const db = open.result
        const tx = db.transaction('files', 'readwrite')
        tx.objectStore('files').put(handle, 'local')
        tx.oncomplete = () => { db.close(); resolve() }
        tx.onerror = () => { db.close(); reject(tx.error) }
      }
    })
    localStorage.setItem('quirenote:session', JSON.stringify({ note: 'A.md', doc: null, local: file.name, page: 3 }))
    localStorage.setItem(`quirenote:page:local:${file.name}:${file.size}`, '3')
  }, pdf)
}

test('a saved device PDF and its page survive refresh', async ({ page }) => {
  await mockVault(page, { 'A.md': 'alpha' })
  await page.goto('/#note=A.md')
  await expect(page.locator('.editor .status')).toHaveText('saved')
  await rememberLocalPdf(page, pdfDocument())
  await page.reload()
  await expect(page.locator('.reader .toolbar .title')).toHaveText('local.pdf')
  await expect(page.locator('.reader .toolbar .muted')).toHaveText('/ 4')
  await expect(page.getByRole('spinbutton', { name: 'Page', exact: true })).toHaveValue('3')
})

test('a late device-file restore cannot replace the document the user just opened', async ({ page }) => {
  await mockVault(page, { 'A.md': 'alpha', 'book.pdf': '' })
  await page.goto('/#note=A.md')
  await expect(page.locator('.editor .status')).toHaveText('saved')
  await rememberLocalPdf(page, pdfDocument())
  await page.addInitScript(() => {
    const getFile = FileSystemFileHandle.prototype.getFile
    FileSystemFileHandle.prototype.getFile = async function () {
      ;(window as any).localReadStarted = true
      await new Promise<void>((resolve) => { (window as any).releaseLocal = resolve })
      return getFile.call(this)
    }
  })
  await page.reload()
  await page.waitForFunction(() => (window as any).localReadStarted)
  await page.getByTitle('book.pdf', { exact: true }).click()
  await expect(page.locator('.reader .toolbar .muted')).toHaveText('/ 4')
  await page.evaluate(() => (window as any).releaseLocal())
  await expect.poll(() => page.evaluate(() => JSON.parse(localStorage.getItem('quirenote:session')!).local)).toBeNull()
  await expect(page.locator('.reader .toolbar .title')).toHaveText('book.pdf')
})

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

for (const preview of [false, true]) {
  test(`escaped wikilink aliases resolve in ${preview ? 'preview' : 'the editor'}`, async ({ page }) => {
    await mockVault(page, { 'A.md': '[[Target\\|alias]]', 'Target.md': 'target content' })
    await page.goto('/#note=A.md')
    await expect(page.locator('.editor .status')).toHaveText('saved')
    if (preview) {
      await page.getByRole('button', { name: 'Preview', exact: true }).click()
      await page.getByRole('link', { name: 'alias', exact: true }).click()
    } else await page.keyboard.press('Control+Enter')
    await expect(page.locator('.crumb')).toHaveText('Target.md')
  })
}

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

test('custom fonts follow the interface, the notes and the code, and survive a reload', async ({ page }) => {
  await mockVault(page, { 'A.md': 'alpha' })
  await page.goto('/#note=A.md')
  await expect(page.locator('.editor .status')).toHaveText('saved')
  await openFonts(page)
  await fontField(page, 'Interface').fill('Fira Sans')
  await fontField(page, 'Notes').fill('Literata')
  await fontField(page, 'Code').fill('Fira Code')
  await page.getByRole('button', { name: 'Done' }).click()
  for (const [selector, family] of [['html', 'Fira Sans'], ['.cm-scroller', 'Literata'], ['.cm-gutters', 'Fira Code']]) {
    expect(await fontOf(page, selector)).toContain(family)
  }
  await page.reload()
  await expect(page.locator('.editor .status')).toHaveText('saved')
  for (const [selector, family] of [['html', 'Fira Sans'], ['.cm-scroller', 'Literata'], ['.cm-gutters', 'Fira Code']]) {
    expect(await fontOf(page, selector)).toContain(family)
  }
  await page.getByRole('button', { name: 'Preview', exact: true }).click()
  expect(await fontOf(page, '.preview article')).toContain('Literata')
})

test('the saved fonts are applied before the app starts, and an injected stack is ignored', async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem(
      'quirenote:fonts',
      JSON.stringify({ ui: "'Fira Sans', var(--font-ui-tail)", editor: 'url(https://evil.example/x)', mono: 7 }),
    )
  })
  // Without the app bundle (it brings the CSS and the dialog) only theme-init.js has run.
  await page.route((url) => url.pathname === '/src/main.ts', (route) => route.abort())
  await page.goto('/')
  const choices = await page.evaluate(() => ({
    ui: document.documentElement.style.getPropertyValue('--font-ui-choice'),
    editor: document.documentElement.style.getPropertyValue('--font-editor-choice'),
    mono: document.documentElement.style.getPropertyValue('--font-mono-choice'),
  }))
  expect(choices).toEqual({ ui: "'Fira Sans', var(--font-ui-tail)", editor: '', mono: '' })
  await expect(page.locator('#app')).toBeEmpty()
})

/** Opens the fonts dialog on a device that lists the given installed fonts. */
async function openFontsWith(page: Page, families: string[]) {
  await page.addInitScript((families) => {
    ;(window as any).queryLocalFonts = async () => families.map((family) => ({ family }))
  }, families)
  await mockVault(page, { 'A.md': 'alpha' })
  await page.goto('/#note=A.md')
  await openFonts(page)
}

test('the font settings suggest the fonts installed on this device', async ({ page }) => {
  await openFontsWith(page, ['Fira Code', 'Fira Sans', 'Fira Sans'])
  await fontField(page, 'Notes').focus()
  const options = page.getByRole('option')
  await expect(options).toHaveText(['Fira Code', 'Fira Sans'])
  // Each name is drawn in its own font, so the choice can be seen before it is made.
  for (const name of ['Fira Code', 'Fira Sans']) {
    expect(await options.filter({ hasText: name }).evaluate((el) => getComputedStyle(el).fontFamily)).toContain(name)
  }
})

test('typing filters the font suggestions, and the arrows with Enter pick one', async ({ page }) => {
  await openFontsWith(page, ['Fira Code', 'Fira Sans', 'Literata', 'Inter'])
  const field = fontField(page, 'Notes')
  await field.focus()
  await expect(page.getByRole('option')).toHaveCount(4)
  await field.fill('FIRA')
  await expect(page.getByRole('option')).toHaveText(['Fira Code', 'Fira Sans'])
  await field.fill('zzz')
  await expect(page.getByRole('listbox')).toBeHidden()
  await field.fill('fira')
  await page.keyboard.press('ArrowDown')
  await page.keyboard.press('ArrowDown')
  await expect(page.getByRole('option', { name: 'Fira Sans' })).toHaveAttribute('aria-selected', 'true')
  await expect(field).toHaveAttribute('aria-activedescendant', /font-editor-option-1/)
  await page.keyboard.press('Enter')
  await expect(field).toHaveValue('Fira Sans')
  await expect(page.getByRole('listbox')).toBeHidden()
  await expect(page.getByRole('dialog', { name: 'Fonts' })).toBeVisible()
  expect(await fontOf(page, '.cm-scroller')).toContain('Fira Sans')
})

test('clicking a font suggestion picks it and keeps one list open at a time', async ({ page }) => {
  await openFontsWith(page, ['Fira Code', 'Fira Sans', 'Literata'])
  await fontField(page, 'Interface').click()
  await expect(page.getByRole('listbox')).toHaveCount(1)
  await fontField(page, 'Code').focus()
  await expect(page.getByRole('listbox')).toHaveCount(1)
  await expect(fontField(page, 'Interface')).toHaveAttribute('aria-expanded', 'false')
  await expect(fontField(page, 'Code')).toHaveAttribute('aria-expanded', 'true')
  await page.getByRole('option', { name: 'Fira Code' }).click()
  await expect(fontField(page, 'Code')).toHaveValue('Fira Code')
  await expect(page.getByRole('listbox')).toBeHidden()
  expect(await fontOf(page, '.cm-gutters')).toContain('Fira Code')
})

test('Escape closes the font suggestions first and the fonts dialog second', async ({ page }) => {
  await openFontsWith(page, ['Fira Code', 'Fira Sans'])
  const dialog = page.getByRole('dialog', { name: 'Fonts' })
  await fontField(page, 'Notes').focus()
  await expect(page.getByRole('listbox')).toBeVisible()
  await page.keyboard.press('Escape')
  await expect(page.getByRole('listbox')).toBeHidden()
  await expect(dialog).toBeVisible()
  await page.keyboard.press('Escape')
  await expect(dialog).toBeHidden()
})

test('the font suggestions use the colors of the app in the dark theme', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('quirenote:theme', 'dark'))
  await openFontsWith(page, ['Fira Code', 'Fira Sans'])
  await fontField(page, 'Notes').focus()
  const list = page.getByRole('listbox')
  await expect(list).toBeVisible()
  const look = await list.evaluate((el) => {
    const probe = document.createElement('div')
    probe.style.background = 'var(--bg)'
    document.body.append(probe)
    const bg = getComputedStyle(probe).backgroundColor
    probe.remove()
    const style = getComputedStyle(el)
    return { bg, listBg: style.backgroundColor, color: style.color, text: getComputedStyle(document.body).color }
  })
  expect(look.listBg).toBe(look.bg)
  expect(look.color).toBe(look.text)
  // Dark, not the pale native popup.
  expect(look.listBg).toBe('rgb(30, 31, 34)')
})

test('without the font access API the settings still take a typed name', async ({ page }) => {
  await page.addInitScript(() => {
    ;(window as any).queryLocalFonts = undefined
  })
  await mockVault(page, { 'A.md': 'alpha' })
  await page.goto('/#note=A.md')
  await openFonts(page)
  await expect(page.getByText('type the name, as in “Inter”')).toBeVisible()
  await fontField(page, 'Notes').fill('Georgia')
  expect(await fontOf(page, '.cm-scroller')).toContain('Georgia')
})

test('the font settings can go back to the default fonts', async ({ page }) => {
  await mockVault(page, { 'A.md': 'alpha' })
  await page.goto('/#note=A.md')
  await openFonts(page)
  await fontField(page, 'Notes').fill('Literata')
  await fontField(page, 'Code').fill('Fira Code')
  expect(await fontOf(page, '.cm-scroller')).toContain('Literata')
  await page.getByRole('button', { name: 'Default font for the code' }).click()
  expect(await fontOf(page, '.cm-gutters')).not.toContain('Fira Code')
  await page.getByRole('button', { name: 'Reset all' }).click()
  expect(await fontOf(page, '.cm-scroller')).toContain('EB Garamond')
  expect(await fontOf(page, '.cm-gutters')).toContain('ui-monospace')
  await page.getByRole('button', { name: 'Done' }).click()
  expect(await page.evaluate(() => localStorage.getItem('quirenote:fonts'))).toBe('{"ui":"","editor":"","mono":""}')
})

test('a dropped character never stays in the field while typing', async ({ page }) => {
  await mockVault(page, { 'A.md': 'alpha' })
  await page.goto('/#note=A.md')
  await openFonts(page)
  const field = fontField(page, 'Interface')
  // Typed one character at a time and in the middle: the field must always show the name alone.
  await field.click()
  await field.pressSequentially('Fira Sans', { delay: 5 })
  await field.press('Home')
  await field.press('ArrowRight')
  await field.pressSequentially('}', { delay: 5 })
  await expect(field).toHaveValue('Fira Sans')
  await field.pressSequentially('X', { delay: 5 })
  await expect(field).toHaveValue('FXira Sans')
  expect(await page.evaluate(() => document.documentElement.style.getPropertyValue('--font-ui-choice'))).toBe(
    "'FXira Sans', var(--font-ui-tail)",
  )
})

test('an injected font stack is ignored when the app starts too', async ({ page }) => {
  await page.addInitScript(() =>
    localStorage.setItem(
      'quirenote:fonts',
      // Passes a plain character whitelist, but is not a list the app itself would ever write.
      JSON.stringify({
        ui: 'expression(alert(1))',
        editor: "'Literata', var(--font-editor-tail), serif",
        mono: "'Fira Code', var(--font-mono-tail)",
      }),
    ),
  )
  await mockVault(page, { 'A.md': 'alpha' })
  await page.goto('/#note=A.md')
  await expect(page.locator('.editor .status')).toHaveText('saved')
  // What the app itself would have written survives; the rest falls back to the default chain.
  expect(await page.evaluate(() => document.documentElement.style.getPropertyValue('--font-mono-choice'))).toBe(
    "'Fira Code', var(--font-mono-tail)",
  )
  expect(await page.evaluate(() => document.documentElement.style.getPropertyValue('--font-ui-choice'))).toBe('')
  expect(await fontOf(page, 'html')).toContain('system-ui')
  expect(await fontOf(page, '.cm-scroller')).toContain('EB Garamond')
  await openFonts(page)
  await expect(fontField(page, 'Interface')).toHaveValue('')
  await expect(fontField(page, 'Notes')).toHaveValue('')
  await expect(fontField(page, 'Code')).toHaveValue('Fira Code')
})

test('the fonts dialog keeps the app shortcuts from acting, and closes with Escape', async ({ page }) => {
  await mockVault(page, { 'A.md': 'alpha' })
  await page.goto('/#note=A.md')
  await expect(page.locator('.editor .status')).toHaveText('saved')
  await openFonts(page)
  // Tab can take the focus out of the dialog; the shortcuts must not act behind it.
  await page.locator('.tree button').first().focus()
  await page.keyboard.press('Control+\\')
  await page.keyboard.press('Control+e')
  await expect(page.locator('.notes-pane')).not.toHaveClass(/hidden/)
  await expect(page.locator('.editor-wrap')).not.toHaveClass(/hidden/)
  await expect(page.getByRole('dialog', { name: 'Fonts' })).toBeVisible()
})

test('closing the fonts dialog gives the focus back to its button', async ({ page }) => {
  await mockVault(page, { 'A.md': 'alpha' })
  await page.goto('/')
  const button = page.getByRole('button', { name: 'Fonts', exact: true })
  await button.click()
  await expect(page.getByRole('dialog', { name: 'Fonts' })).toBeVisible()
  await page.keyboard.press('Escape')
  await expect(page.getByRole('dialog', { name: 'Fonts' })).toBeHidden()
  await expect(button).toBeFocused()
})

test('a font name typed with CSS punctuation keeps only the name', async ({ page }) => {
  await mockVault(page, { 'A.md': 'alpha' })
  await page.goto('/#note=A.md')
  await openFonts(page)
  const field = fontField(page, 'Interface')
  await field.fill('Inter; } body { display: none }')
  await expect(field).toHaveValue('Inter body display none')
  expect(await page.evaluate(() => document.documentElement.style.getPropertyValue('--font-ui-choice'))).toBe(
    "'Inter body display none', var(--font-ui-tail)",
  )
  expect(await fontOf(page, 'html')).toContain('Inter body display none')
  expect(await page.evaluate(() => getComputedStyle(document.body).display)).not.toBe('none')
})

test('the fonts dialog uses the app colors and the interface font, not those of pdf.js', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('quirenote:theme', 'dark'))
  await mockVault(page, { 'A.md': 'alpha' })
  await page.goto('/#note=A.md')
  await openFonts(page)
  await fontField(page, 'Interface').fill('Fira Sans')
  const look = () =>
    page.evaluate(() => {
      const title = getComputedStyle(document.querySelector('[role=dialog] h2')!)
      return { color: title.color, font: title.fontFamily, text: getComputedStyle(document.body).color }
    })
  const { color, font, text } = await look()
  expect(color).toBe(text)
  expect(font).toContain('Fira Sans')
})

test('the font settings say when a typed font is not on this device', async ({ page }) => {
  await mockVault(page, { 'A.md': 'alpha' })
  await page.goto('/#note=A.md')
  await openFonts(page)
  const notice = page.getByText('Not found on this device')
  await fontField(page, 'Notes').fill('No Such Font Anywhere')
  await expect(notice).toBeVisible()
  // The app's own font counts as available, even before the browser draws it.
  await fontField(page, 'Notes').fill('EB Garamond')
  await expect(notice).toBeHidden()
  await fontField(page, 'Code').fill('monospace')
  await expect(notice).toBeHidden()
})

test('a wrapped list item stays aligned when the notes font changes', async ({ page }) => {
  await page.setViewportSize({ width: 900, height: 700 })
  await mockVault(page, { 'A.md': `- ${'a long list item that wraps '.repeat(12)}` })
  await page.goto('/#note=A.md')
  await expect(page.locator('.editor .status')).toHaveText('saved')
  const indent = () => page.locator('.cm-line').first().evaluate((line) => (line as HTMLElement).style.paddingLeft)
  const before = await indent()
  await openFonts(page)
  await fontField(page, 'Notes').fill('monospace')
  await page.getByRole('button', { name: 'Done' }).click()
  await expect.poll(indent).not.toBe(before)
  const after = await indent()
  await page.reload()
  await expect(page.locator('.editor .status')).toHaveText('saved')
  expect(await indent()).toBe(after)
})
