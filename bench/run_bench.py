#!/usr/bin/env python3
"""Benchmark driver: runs every scenario against every server, one server at a time, fresh
process and fresh vault copy per run, and records server-side CPU and memory from /proc.

    run_bench.py --server go=/tmp/qbench/bin/quirenote-go --server rs=/path/quirenote-rs \
                 --reps 3 --out /tmp/qbench/results/raw.json [--scenarios conflict,rmw,...]

SAFETY: servers only ever see copies under /tmp/qbench/run, a throw-away HOME/XDG_CONFIG_HOME and
ports 9130-9199. Server processes are pinned to CPUs 0-7 and the load generator to CPUs 8-15
(`taskset`), so the two never fight for the same cores. Standard library only.
"""
import argparse
import json
import os
import shutil
import signal
import subprocess
import sys
import time
import urllib.request

ROOT = "/tmp/qbench"
DATA = f"{ROOT}/data"
RUN = f"{ROOT}/run"
LOADGEN = f"{ROOT}/bin/loadgen"
SERVER_CPUS, LOAD_CPUS = "0-7", "8-15"
TICK = os.sysconf("SC_CLK_TCK")
PORT = 9130

# name -> (dataset, [ (label, loadgen args...) ])
SCENARIOS = {
    "conflict": ("small", [(f"conflict-{n}", ["conflict", "--clients", str(n), "--size", str(16 << 10)]) for n in (50, 200, 1000)]),
    # 64 KiB bodies: same scenario, but on loopback they trigger ~200 ms TCP stalls (delayed ACK /
    # window update) that depend on how fast each server drains the socket; kept to show the effect.
    "conflict64k": ("small", [(f"conflict64k-{n}", ["conflict", "--clients", str(n), "--size", str(64 << 10)]) for n in (50, 200, 1000)]),
    "rmw": ("small", [("rmw-50x10", ["rmw", "--workers", "50", "--writes", "10"]),
                      ("rmw-200x5", ["rmw", "--workers", "200", "--writes", "5"]),
                      ("rmw-1000x2", ["rmw", "--workers", "1000", "--writes", "2"])]),
    "writes": ("small", [("writes-64", ["writes", "--clients", "64", "--duration", "10s", "--size", "4096"])]),
    "reads": ("large", [("read-note", ["reads", "--kind", "note", "--index", "{idx}/notes.txt", "--clients", "64", "--duration", "10s"]),
                        ("read-tree", ["reads", "--kind", "tree", "--clients", "16", "--duration", "10s"]),
                        ("read-search", ["reads", "--kind", "search", "--index", "{idx}/queries.txt", "--clients", "16", "--duration", "10s"]),
                        ("read-backlinks", ["reads", "--kind", "backlinks", "--index", "{idx}/linknotes.txt", "--clients", "16", "--duration", "10s"])]),
    "sse": ("small", [("sse-500", ["sse", "--clients", "500", "--events", "20"]),
                      ("sse-2000", ["sse", "--clients", "2000", "--events", "20"])]),
    "consistency": ("small", [("consistency", ["consistency", "--n", "400", "--concurrency", "100"])]),
}


def read_status(pid, key):
    try:
        for line in open(f"/proc/{pid}/status"):
            if line.startswith(key + ":"):
                return int(line.split()[1])
    except OSError:
        pass
    return -1


def cpu_seconds(pid):
    """(self user+sys, waited-for children user+sys) in seconds."""
    try:
        f = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()
    except OSError:
        return (-1, -1)
    ut, st, cut, cst = (int(x) for x in f[11:15])
    return ((ut + st) / TICK, (cut + cst) / TICK)


def http_ok(url, timeout=0.5):
    try:
        with urllib.request.urlopen(url, timeout=timeout) as r:
            r.read(1)
            return r.status
    except Exception:
        return None


def start_server(binary, port, vault, home, log):
    for p in (vault, home, log):
        assert p.startswith(RUN + "/"), p
    assert 9100 <= port <= 9199
    os.makedirs(home + "/.config", exist_ok=True)
    env = {k: v for k, v in os.environ.items() if k not in ("QUIRENOTE_VAULT", "NOTAS_VAULT", "QUIRENOTE_ADDR", "NOTAS_ADDR")}
    env.update(HOME=home, XDG_CONFIG_HOME=home + "/.config")
    logf = open(log, "wb")
    t0 = time.monotonic()
    p = subprocess.Popen(["taskset", "-c", SERVER_CPUS, binary, "-addr", f"127.0.0.1:{port}", "-vault", vault],
                         env=env, stdout=logf, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL)
    listen_ms = tree_ms = None
    while time.monotonic() - t0 < 30:
        if listen_ms is None and http_ok(f"http://127.0.0.1:{port}/api/setup") == 200:
            listen_ms = (time.monotonic() - t0) * 1000
        if listen_ms is not None and http_ok(f"http://127.0.0.1:{port}/api/tree", 5) == 200:
            tree_ms = (time.monotonic() - t0) * 1000
            break
        time.sleep(0.002)
    out = open(log).read()
    if f"vault={vault}" not in out:
        p.kill()
        raise SystemExit(f"startup log does not name the temporary vault {vault}:\n{out}")
    return p, listen_ms, tree_ms


def stop_server(p):
    p.send_signal(signal.SIGTERM)
    try:
        p.wait(10)
    except subprocess.TimeoutExpired:
        p.kill()
        p.wait()


def run_one(server, binary, scenario, label, lg_args, dataset, rep):
    global PORT
    PORT = 9130 + (PORT - 9130 + 1) % 60
    name = f"{server}-{label}-{rep}"
    base = f"{RUN}/{name}"
    shutil.rmtree(base, ignore_errors=True)
    os.makedirs(base)
    vault = f"{base}/vault"
    subprocess.run(["cp", "-a", f"{DATA}/{dataset}/vault", vault], check=True)
    p, listen_ms, tree_ms = start_server(binary, PORT, vault, f"{base}/home", f"{base}/server.log")
    try:
        time.sleep(1.5)
        idle_rss = read_status(p.pid, "VmRSS")
        hwm0 = read_status(p.pid, "VmHWM")
        thr0 = read_status(p.pid, "Threads")
        cpu0 = cpu_seconds(p.pid)
        args = [a.replace("{idx}", f"{DATA}/{dataset}") for a in lg_args]
        cmd = ["taskset", "-c", LOAD_CPUS, LOADGEN, args[0], "--url", f"http://127.0.0.1:{PORT}"] + args[1:]
        if args[0] in ("conflict", "rmw", "sse", "consistency"):
            cmd += ["--vault", vault]
        if args[0] == "sse":
            cmd += ["--pid", str(p.pid)]
        t0 = time.monotonic()
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=600)
        wall = time.monotonic() - t0
        if r.returncode != 0:
            raise SystemExit(f"loadgen failed: {r.stderr[-2000:]}")
        res = json.loads(r.stdout.strip().splitlines()[-1])
        cpu1 = cpu_seconds(p.pid)
        rss_end = read_status(p.pid, "VmRSS")
        hwm = read_status(p.pid, "VmHWM")
        thr1 = read_status(p.pid, "Threads")
        time.sleep(3)
        rss_after = read_status(p.pid, "VmRSS")
        if p.poll() is not None:
            raise SystemExit(f"{server} died during {label}: {open(base + '/server.log').read()[-1500:]}")
        leftovers = subprocess.run(["find", vault, "-name", "*.tmp"], capture_output=True, text=True).stdout.split()
        res["server"] = {
            "listen_ms": listen_ms, "first_tree_ms": tree_ms,
            "rss_idle_kb": idle_rss, "vmhwm_before_kb": hwm0, "vmhwm_kb": hwm, "rss_end_kb": rss_end, "rss_3s_after_kb": rss_after,
            "cpu_s": cpu1[0] - cpu0[0], "child_cpu_s": cpu1[1] - cpu0[1], "threads_before": thr0, "threads_after": thr1,
            "wall_s": wall, "tmp_files_left": len(leftovers),
        }
        return res
    finally:
        stop_server(p)
        shutil.rmtree(base, ignore_errors=True)


def startup_only(server, binary, dataset, rep):
    global PORT
    PORT = 9130 + (PORT - 9130 + 1) % 60
    base = f"{RUN}/{server}-startup-{dataset}-{rep}"
    shutil.rmtree(base, ignore_errors=True)
    os.makedirs(base)
    subprocess.run(["cp", "-a", f"{DATA}/{dataset}/vault", f"{base}/vault"], check=True)
    p, listen_ms, tree_ms = start_server(binary, PORT, f"{base}/vault", f"{base}/home", f"{base}/server.log")
    try:
        time.sleep(1.5)
        return {"scenario": "startup", "dataset": dataset, "server": {"listen_ms": listen_ms, "first_tree_ms": tree_ms, "rss_idle_kb": read_status(p.pid, "VmRSS"),
                                                                      "vmhwm_kb": read_status(p.pid, "VmHWM"), "threads_before": read_status(p.pid, "Threads")}}
    finally:
        stop_server(p)
        shutil.rmtree(base, ignore_errors=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--server", action="append", required=True, help="name=binary (repeatable)")
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--out", required=True)
    ap.add_argument("--scenarios", default="startup," + ",".join(SCENARIOS))
    a = ap.parse_args()
    servers = [tuple(s.split("=", 1)) for s in a.server]
    wanted = a.scenarios.split(",")
    os.makedirs(os.path.dirname(a.out), exist_ok=True)
    os.makedirs(RUN, exist_ok=True)
    results = json.load(open(a.out)) if os.path.exists(a.out) else []

    def save():
        json.dump(results, open(a.out, "w"), indent=1)

    for rep in range(a.reps):
        if "startup" in wanted:
            for ds in ("small", "large"):
                for name, binary in servers:
                    r = startup_only(name, binary, ds, rep)
                    r.update(name=name, label=f"startup-{ds}", rep=rep)
                    results.append(r)
                    print(f"[rep {rep}] {name:3} startup-{ds}: listen {r['server']['listen_ms']:.1f} ms, first /api/tree {r['server']['first_tree_ms']:.1f} ms, idle RSS {r['server']['rss_idle_kb']/1024:.1f} MB", flush=True)
                    save()
        for scen, (ds, runs) in SCENARIOS.items():
            if scen not in wanted:
                continue
            for label, lg in runs:
                for name, binary in servers:  # alternate servers inside each repetition
                    r = run_one(name, binary, scen, label, lg, ds, rep)
                    r.update(name=name, label=label, rep=rep)
                    results.append(r)
                    s = r["server"]
                    print(f"[rep {rep}] {name:3} {label}: cpu {s['cpu_s']:.2f}s, HWM {s['vmhwm_kb']/1024:.1f} MB, rss {s['rss_idle_kb']/1024:.1f}->{s['rss_end_kb']/1024:.1f}->{s['rss_3s_after_kb']/1024:.1f} MB", flush=True)
                    save()
    print("done ->", a.out)


if __name__ == "__main__":
    main()
