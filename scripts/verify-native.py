#!/usr/bin/env python3
"""Drive a real native window against a synthetic vault: save, restore and external change."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

import shlex
import signal
import socket
import urllib.request
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parent.parent

def wl_env():
    env = dict(os.environ)
    for line in Path(args.wayland_env).read_text().splitlines():
        if line.startswith('export '):
            for pair in shlex.split(line[7:]):
                key, value = pair.split('=', 1)
                env[key] = value
        elif line.startswith('unset '):
            for key in line[6:].split():
                env.pop(key, None)
    return env

def drive(line, pause):
    fd = os.open(args.input_fifo, os.O_WRONLY | os.O_NONBLOCK)
    try:
        os.write(fd, (line + '\n').encode())
    finally:
        os.close(fd)
    time.sleep(pause)

def start_server(kind, port, vault, home):
    config = Path(home) / '.config/quirenote'
    config.mkdir(parents=True)
    (config / 'config.toml').write_text('vault = ' + json.dumps(vault) + '\n')
    with socket.socket() as probe:
        probe.bind(('127.0.0.1', port))
    env = dict(os.environ, XDG_CONFIG_HOME=str(config.parent))
    process = subprocess.Popen([args.rust_server, '-vault', vault, '-addr', f'127.0.0.1:{port}'],
        env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
    for _ in range(100):
        if process.poll() is not None:
            raise RuntimeError('Backend exited during startup')
        try:
            with urllib.request.urlopen(f'http://127.0.0.1:{port}/api/tree', timeout=1) as response:
                if response.status == 200:
                    return process
        except OSError:
            time.sleep(.2)
    stop(process)
    raise RuntimeError('Backend startup timed out')

def start_client(kind, port, directory, note, doc):
    argv = [args.gui, '--server', f'http://127.0.0.1:{port}', '--state-dir', str(Path(directory) / 'state')]
    if note:
        argv += ['--note', note]
    if doc:
        argv += ['--doc', doc]
    return subprocess.Popen(argv, env=harness.ENV, stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL, start_new_session=True), None

def stop(process):
    if process and process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()

harness = SimpleNamespace(ROOT=ROOT, wl_env=wl_env, start_server=start_server,
    start_client=start_client, stop=stop, key=lambda key: drive('key ' + key, .6),
    typ=lambda text: drive('type ' + text, .4 + .03 * len(text)))

parser = argparse.ArgumentParser()
parser.add_argument('--out', required=True)
parser.add_argument('--rust-server', default=str(ROOT / 'bin/quirenote'))
parser.add_argument('--gui', default=str(ROOT / 'bin/quirenote-gui'))
parser.add_argument('--port', type=int, default=9183)
parser.add_argument('--wayland-env', required=True, help='exports for a private test compositor')
parser.add_argument('--input-fifo', required=True, help='private virtual keyboard FIFO')
parser.add_argument('--documents', required=True, help='synthetic folder with livro.pdf and livro.epub')
args = parser.parse_args()
output = Path(args.out)
output.mkdir(parents=True, exist_ok=True)
harness.args = args
harness.ENV = harness.wl_env()
tmp = Path(tempfile.mkdtemp(prefix='native-flow-', dir=output))
(tmp / 'vault').mkdir()
(tmp / 'home').mkdir()
(tmp / 'vault/Livros').mkdir()
for name in ('livro.pdf', 'livro.epub'):
    shutil.copy2(Path(args.documents) / name, tmp / 'vault/Livros' / name)
note = tmp / 'vault/Check.md'
note.write_text('# Interface check\n\nORIGINAL\n')
server = client = None
results = []
def saved(marker):
    deadline = time.monotonic() + 10
    while marker not in note.read_text():
        if time.monotonic() > deadline:
            raise RuntimeError(f'Autosave did not persist {marker}')
        time.sleep(.2)
def append(marker):
    harness.key('G')
    harness.key('o')
    harness.typ(marker)
    harness.key('Escape')
    saved(marker)
def shot(name):
    subprocess.run(['grim', str(output / (name + '.png'))], env=harness.ENV, check=True)
try:
    server = harness.start_server('rust', args.port, str(tmp / 'vault'), str(tmp / 'home'))
    client, _ = harness.start_client('gui', args.port, str(tmp), 'Check.md', None)
    time.sleep(8)
    append('GUI-SAVE-MARKER')
    assert 'ORIGINAL' in note.read_text()
    shot('1-saved')
    results.append('Keyboard edit autosaved to the vault')
    harness.stop(client)
    client, _ = harness.start_client('gui', args.port, str(tmp), None, None)
    time.sleep(8)
    state = json.loads((tmp / 'state/state.json').read_text())
    assert state['session']['note'] == 'Check.md'
    shot('2-restored')
    results.append('A fresh window restored the selected note')
    external = tmp / 'vault/.external.tmp'
    external.write_text(note.read_text() + '\nSSE-MARKER\n')
    os.replace(external, note)
    time.sleep(3)
    shot('3-external')
    append('AFTER-SSE-MARKER')
    assert all(marker in note.read_text() for marker in ('ORIGINAL', 'GUI-SAVE-MARKER', 'SSE-MARKER', 'AFTER-SSE-MARKER'))
    shot('4-after-external-save')
    results.append('An external replacement reached the editor and a subsequent edit preserved it')
    harness.stop(client)
    client, _ = harness.start_client('gui', args.port, str(tmp), 'Check.md', 'Livros/livro.pdf')
    time.sleep(8)
    for _ in range(3):
        harness.key('alt+shift+Down')
    time.sleep(2)
    state = json.loads((tmp / 'state/state.json').read_text())
    assert state['session']['doc'] == 'Livros/livro.pdf'
    assert state['session']['page'] == 4, state['session']
    shot('5-pdf-page-4')
    results.append('PDF page navigation persisted page 4')
    harness.stop(client)
    client, _ = harness.start_client('gui', args.port, str(tmp), 'Check.md', 'Livros/livro.epub')
    deadline = time.monotonic() + 20
    while True:
        if client.poll() is not None:
            raise RuntimeError('EPUB client exited')
        state = json.loads((tmp / 'state/state.json').read_text())
        if state['session']['doc'] == 'Livros/livro.epub' and state.get('epub', {}).get('Livros/livro.epub'):
            break
        if time.monotonic() > deadline:
            raise RuntimeError('EPUB reading position did not become ready')
        time.sleep(.2)
    time.sleep(2)
    harness.key('ctrl+e')
    shot('6-epub-and-preview')
    results.append('EPUB reading position loaded with the Markdown preview open')
    (output / 'final-state.json').write_text(json.dumps(state, indent=2))
    (output / 'saved-note.md').write_text(note.read_text())
    (output / 'result.json').write_text(json.dumps({'passed': results}, indent=2))
    print(f'{len(results)}/{len(results)} native flow checks passed')
finally:
    harness.stop(client)
    harness.stop(server)
    shutil.rmtree(tmp)
