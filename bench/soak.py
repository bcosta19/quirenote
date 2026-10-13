#!/usr/bin/env python3
"""Mixed-load soak: does RSS stay flat? Runs reads (note/tree/search/backlinks) and saves concurrently
for --seconds against ONE server on a copy of the large vault and samples VmRSS every 5 s.

    soak.py --server go=/tmp/qbench/bin/quirenote-go --seconds 90
Prints one line per sample; same confinement rules as run_bench.py (copy under /tmp/qbench/run,
ports 9130-9199, throw-away HOME).
"""
import argparse
import os
import shutil
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import run_bench as rb  # noqa: E402


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--server", action="append", required=True)
    ap.add_argument("--seconds", type=int, default=90)
    ap.add_argument("--env", action="append", default=[], help="KEY=VALUE for the server process (repeatable)")
    a = ap.parse_args()
    for spec in a.server:
        name, binary = spec.split("=", 1)
        base = f"{rb.RUN}/soak-{name}"
        shutil.rmtree(base, ignore_errors=True)
        os.makedirs(base)
        subprocess.run(["cp", "-a", f"{rb.DATA}/large/vault", f"{base}/vault"], check=True)
        port = 9190
        extra = dict(e.split("=", 1) for e in a.env)
        p, _, _ = rb.start_server(binary, port, f"{base}/vault", f"{base}/home", f"{base}/server.log", extra)
        url = f"http://127.0.0.1:{port}"
        d = f"{rb.DATA}/large"
        dur = f"{a.seconds}s"
        loads = [
            ["reads", "--kind", "note", "--index", f"{d}/notes.txt", "--clients", "32", "--duration", dur, "--warmup", "0s"],
            ["reads", "--kind", "tree", "--clients", "2", "--duration", dur, "--warmup", "0s"],
            ["reads", "--kind", "search", "--index", f"{d}/queries.txt", "--clients", "4", "--duration", dur, "--warmup", "0s"],
            ["reads", "--kind", "backlinks", "--index", f"{d}/linknotes.txt", "--clients", "4", "--duration", dur, "--warmup", "0s"],
            ["writes", "--clients", "8", "--duration", dur],
        ]
        procs = [subprocess.Popen(["taskset", "-c", rb.LOAD_CPUS, rb.LOADGEN, l[0], "--url", url] + l[1:], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) for l in loads]
        series = []
        t0 = time.time()
        while time.time() - t0 < a.seconds + 2:
            time.sleep(5)
            series.append(rb.read_status(p.pid, "VmRSS") / 1024)
        for pr in procs:
            pr.wait()
        time.sleep(5)
        series.append(rb.read_status(p.pid, "VmRSS") / 1024)
        hwm = rb.read_status(p.pid, "VmHWM") / 1024
        rb.stop_server(p)
        shutil.rmtree(base, ignore_errors=True)
        print(f"{name}{(' ' + ' '.join(a.env)) if a.env else ''}: RSS MB every 5 s: " + " ".join(f"{x:.0f}" for x in series) + f" | peak {hwm:.0f} MB (last value = 5 s after the load stopped)")


if __name__ == "__main__":
    main()
