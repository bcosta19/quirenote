import { mount } from 'svelte'
import './lib/fonts'
import './app.css'
import App from './App.svelte'
import Setup from './lib/Setup.svelte'
import { getSetup, type SetupStatus } from './lib/api'

// First run (no vault configured yet): show the setup screen instead of the app.
const target = document.getElementById('app')!
const status: SetupStatus = await getSetup().catch(() => ({ configured: true }))
const app = status.configured ? mount(App, { target }) : mount(Setup, { target, props: { status } })

export default app
