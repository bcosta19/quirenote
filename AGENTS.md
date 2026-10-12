# Verification

- Backend and static analysis: `make check` runs gofmt, go vet, staticcheck, errcheck, nilaway, Go tests with the race detector, and Svelte/TypeScript checks.
- Frontend production build: `cd web && npm run build`.
- Browser test/config typecheck with TypeScript 6: `cd web && npx tsc --ignoreConfig --noEmit --skipLibCheck --target ES2023 --module ESNext --moduleResolution Bundler --types node playwright.config.ts tests/review.spec.ts`.
- Frontend regressions: `cd web && npm test`. The Playwright configuration starts its own Vite server on port 5187 and mocks all API requests; tests do not access the real notes vault.
- To use the system Chromium on this Linux environment: `cd web && PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH=/usr/bin/chromium npm test`.
- Alternatively, install Playwright's matching browser with `cd web && npx playwright install chromium` before `npm test`.
- Repeat browser checks with `npm test -- --repeat-each=3`. Run these separately from static analysis/build jobs; concurrent verification produced PDF loading timeouts in this environment.
- Stress save/trash regressions with `go test ./internal/vault -run 'TestConcurrentSavesRejectStaleVersions|TestTrashKeepsEveryVersionWithTheSameName' -count=30`.
- The browser suite covers delayed saves, save errors/conflicts, stale external-change responses, failed note loads, escaped wikilinks, explicit PDF page links, and IndexedDB-backed local PDF restoration.
