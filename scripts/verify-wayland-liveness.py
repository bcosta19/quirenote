#!/usr/bin/env python3
"""Regress EGL startup: a hidden window must still answer Wayland's close event.

Use a private Sway instance whose config assigns quirenote-gui to workspace 1
and contains `no_focus [app_id="quirenote-gui"]`. The test stays on workspace 2.
All notes, configuration, and processes belong to this test.
"""
import argparse
import json
import os
from pathlib import Path
import shlex
import socket
import subprocess
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--gui', default=str(ROOT / 'bin/quirenote-gui'))
parser.add_argument('--server', default=str(ROOT / 'bin/quirenote'))
parser.add_argument('--wayland-env', required=True)
parser.add_argument('--swaymsg', default='swaymsg')
parser.add_argument('--out', required=True)
parser.add_argument('--close-timeout', type=float, default=2)
args = parser.parse_args()
env = dict(os.environ)
for line in Path(args.wayland_env).read_text().splitlines():
    if line.startswith('export '):
        for pair in shlex.split(line[7:]):
            key, value = pair.split('=', 1)
            env[key] = value
    elif line.startswith('unset '):
        for key in line[6:].split():
            env.pop(key, None)
if env.get('XDG_RUNTIME_DIR') == os.environ.get('XDG_RUNTIME_DIR') or not env.get('SWAYSOCK'):
    parser.error('A private compositor environment is required; do not use the desktop session')

def ipc(*arguments):
    result = subprocess.run([args.swaymsg, *arguments], env=env, capture_output=True,
                            text=True, check=True, timeout=5)
    return json.loads(result.stdout)

def find_window(node, pid):
    if node.get('pid') == pid and node.get('app_id') == 'quirenote-gui':
        return node
    for child in node.get('nodes', []) + node.get('floating_nodes', []):
        found = find_window(child, pid)
        if found:
            return found
    return None

out = Path(args.out)
out.mkdir(parents=True, exist_ok=True)
result = {'hidden_workspace': 1, 'visible_workspace': 2, 'close_timeout_seconds': args.close_timeout}
server = gui = None
try:
    with tempfile.TemporaryDirectory(prefix='liveness-', dir=out) as directory:
        tmp = Path(directory)
        vault = tmp / 'vault'
        vault.mkdir()
        (vault / 'Check.md').write_text('# Startup regression\n\nSynthetic content.\n')
        config = tmp / 'config/quirenote'
        config.mkdir(parents=True)
        (config / 'config.toml').write_text('vault = ' + json.dumps(str(vault)) + '\n')
        with socket.socket() as probe:
            probe.bind(('127.0.0.1', 0))
            port = probe.getsockname()[1]
        server = subprocess.Popen([args.server, '-vault', str(vault), '-addr', f'127.0.0.1:{port}'],
                                  env=dict(env, XDG_CONFIG_HOME=str(config.parent)),
                                  stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        deadline = time.monotonic() + 10
        while True:
            if server.poll() is not None or time.monotonic() >= deadline:
                raise RuntimeError('Synthetic backend did not start')
            try:
                with urllib.request.urlopen(f'http://127.0.0.1:{port}/api/tree', timeout=1):
                    break
            except OSError:
                time.sleep(.1)
        ipc('workspace 2')
        with (out / 'gui.log').open('w') as log:
            gui = subprocess.Popen([args.gui, '--server', f'http://127.0.0.1:{port}',
                                    '--state-dir', str(tmp / 'state'), '--note', 'Check.md'],
                                   env=env, stdout=log, stderr=log)
        deadline = time.monotonic() + 10
        while not find_window(ipc('-t', 'get_tree'), gui.pid):
            if gui.poll() is not None or time.monotonic() >= deadline:
                raise RuntimeError('Native window did not map')
            time.sleep(.05)
        workspaces = ipc('-t', 'get_workspaces')
        if not any(w['name'] == '2' and w['visible'] for w in workspaces):
            raise RuntimeError('Compositor focused the test window; configure no_focus first')
        if not any(w['name'] == '1' and not w['visible'] for w in workspaces):
            raise RuntimeError('The test window is not on a hidden workspace')
        time.sleep(1)
        requested = time.monotonic()
        ipc(f'[pid={gui.pid}] kill')
        try:
            code = gui.wait(timeout=args.close_timeout)
            result.update(passed=code == 0, exit_code=code,
                          close_seconds=round(time.monotonic() - requested, 3))
        except subprocess.TimeoutExpired:
            result.update(passed=False, reason='Hidden window did not process the Wayland close event')
finally:
    for process in (gui, server):
        if process and process.poll() is None:
            process.terminate()
            process.wait(timeout=10)
    (out / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps(result))
raise SystemExit(0 if result.get('passed') else 1)
