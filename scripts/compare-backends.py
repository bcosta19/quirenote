#!/usr/bin/env python3
"""Compare corrected Go and the Rust experiment using its reviewed benchmark harness.

Requires --bench-dir from branch bench/rust-axum-vs-go and prebuilt binaries.
All server data/configuration lives in a newly created /tmp/qbench directory.
No dependency installation, real vault access, or service management.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import time


def record(path, value):
    temp = path.with_suffix(".tmp")
    temp.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")
    temp.replace(path)


def assert_correct(row):
    for key in ("lost_updates", "errors", "non_200", "transport_errors", "leftover_tmp"):
        if row.get(key, 0) != 0:
            raise RuntimeError(f"{row['name']} {row['label']}: {key}={row[key]}")
    if row.get("scenario") == "conflict":
        if row.get("accepted") != 1 or row.get("integrity") != "ok":
            raise RuntimeError(f"invalid conflict outcome: {row['name']} {row['label']}")
        if row.get("other_status"):
            raise RuntimeError(f"unexpected conflict status: {row['other_status']}")
    if row.get("scenario") == "sse":
        if row["connected"] != row["clients_requested"] or row["events_all_delivered"] != row["events"]:
            raise RuntimeError(f"incomplete SSE delivery: {row['name']}")
    if row.get("server", {}).get("tmp_files_left", 0):
        raise RuntimeError(f"temporary writes left behind: {row['name']} {row['label']}")
    failed = [c for c in row.get("checks", []) if not c["ok"]]
    if failed:
        raise RuntimeError(f"{row['name']} consistency: {failed}")


def soak(rb, binary, name, rep, seconds):
    base = Path(tempfile.mkdtemp(prefix=f"soak-{name}-{rep}-", dir=rb.RUN))
    shutil.copytree(Path(rb.DATA) / "large/vault", base / "vault")
    p, _, _ = rb.start_server(binary, 9189, str(base / "vault"), str(base / "home"), str(base / "server.log"))
    loads, logs = [], []
    idx = Path(rb.DATA) / "large"
    specs = [
        ["reads", "--kind", "note", "--index", str(idx / "notes.txt"), "--clients", "32", "--warmup", "0s"],
        ["reads", "--kind", "tree", "--clients", "2", "--warmup", "0s"],
        ["reads", "--kind", "search", "--index", str(idx / "queries.txt"), "--clients", "4", "--warmup", "0s"],
        ["reads", "--kind", "backlinks", "--index", str(idx / "linknotes.txt"), "--clients", "4", "--warmup", "0s"],
        ["writes", "--clients", "8"],
    ]
    samples = []
    try:
        time.sleep(1.5)
        idle = rb.read_status(p.pid, "VmRSS")
        cpu0 = rb.cpu_seconds(p.pid)
        for i, spec in enumerate(specs):
            log = (base / f"load-{i}.json").open("w")
            logs.append(log)
            args = ["taskset", "-c", rb.LOAD_CPUS, rb.LOADGEN, spec[0], "--url", "http://127.0.0.1:9189"]
            args += spec[1:] + ["--duration", f"{seconds}s"]
            loads.append(subprocess.Popen(args, stdout=log, stderr=subprocess.PIPE))
        start = time.monotonic()
        while time.monotonic() - start < seconds:
            time.sleep(2)
            if p.poll() is not None:
                raise RuntimeError("server exited during mixed load")
            samples.append({"seconds": round(time.monotonic() - start, 2), "rss_kib": rb.read_status(p.pid, "VmRSS")})
        results = []
        for i, child in enumerate(loads):
            _, err = child.communicate(timeout=30)
            logs[i].close()
            if child.returncode:
                raise RuntimeError(f"mixed-load client failed: {err.decode()[:500]}")
            result = json.loads((base / f"load-{i}.json").read_text().strip().splitlines()[-1])
            assert_correct(dict(result, name=name, label="mixed"))
            results.append(result)
        cpu1 = rb.cpu_seconds(p.pid)
        peak = rb.read_status(p.pid, "VmHWM")
        time.sleep(5)
        after = rb.read_status(p.pid, "VmRSS")
        return {"name": name, "rep": rep, "seconds": seconds, "idle_rss_kib": idle, "peak_rss_kib": peak,
                "rss_5s_after_kib": after, "cpu_s": cpu1[0] - cpu0[0], "samples": samples, "loads": results}
    finally:
        for child in loads:
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
        for log in logs:
            log.close()
        rb.stop_server(p)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--bench-dir", type=Path, required=True)
    ap.add_argument("--go", required=True)
    ap.add_argument("--rust", required=True)
    ap.add_argument("--loadgen", required=True)
    ap.add_argument("--reps", type=int, default=4)
    ap.add_argument("--seconds", type=int, default=5)
    ap.add_argument("--soak-seconds", type=int, default=90)
    ap.add_argument("--soak-reps", type=int, default=2)
    ap.add_argument("--resume-dir", type=Path, help="resume a previous comparison under /tmp/qbench")
    a = ap.parse_args()
    if min(a.reps, a.seconds, a.soak_seconds) < 1 or a.soak_reps < 0:
        ap.error("repetitions/durations must be positive; soak-reps may be zero")
    # Use runtime/allocator defaults for both backends and the client, regardless
    # of tuning inherited from the interactive shell. This affects this process only.
    for key in ["GOMAXPROCS", "GOMEMLIMIT", "GOGC", "MALLOC_ARENA_MAX", "MALLOC_CONF", "QUIRENOTE_RS_COARSE_MTIME"]:
        os.environ.pop(key, None)
    a.bench_dir = a.bench_dir.resolve()
    if a.resume_dir:
        root = a.resume_dir.resolve(strict=True)
        if not root.is_relative_to(Path("/tmp/qbench").resolve()) or not root.name.startswith("go-fixed-vs-rust-"):
            ap.error("resume directory must be a comparison under /tmp/qbench")
    else:
        Path("/tmp/qbench").mkdir(exist_ok=True)
        root = Path(tempfile.mkdtemp(prefix="go-fixed-vs-rust-", dir="/tmp/qbench"))
    spec = importlib.util.spec_from_file_location("backend_bench", a.bench_dir / "run_bench.py")
    rb = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(rb)
    rb.DATA, rb.RUN, rb.LOADGEN = str(root / "data"), str(root / "run"), str(Path(a.loadgen).resolve())
    Path(rb.RUN).mkdir(exist_ok=True)
    original_start = rb.start_server

    def checked_start(binary, port, vault, home, log, extra_env=None):
        # Reuse TIME_WAIT sockets, as both backends do, but reject an occupied
        # listener before any HTTP operation can reach another application.
        with socket.socket() as probe:
            probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            probe.bind(("127.0.0.1", port))
        p, listen, tree = original_start(binary, port, vault, home, log, extra_env)
        if p.poll() is not None or listen is None or tree is None:
            rb.stop_server(p)
            raise RuntimeError("temporary backend did not start successfully")
        return p, listen, tree

    rb.start_server = checked_start
    servers = [("go", str(Path(a.go).resolve())), ("rs", str(Path(a.rust).resolve()))]
    print(f"Artifacts: {root}", flush=True)
    meta = {"repetitions": a.reps, "timed_seconds": a.seconds, "read_warmup_seconds": 1,
            "server_cpus": rb.SERVER_CPUS, "client_cpus": rb.LOAD_CPUS, "filesystem": "tmpfs",
            "soak_seconds": a.soak_seconds, "soak_repetitions": a.soak_reps,
            "runtime_tuning": "defaults; Go/allocator/coarse-mtime overrides removed",
            "binaries": {n: {"path": b, "sha256": hashlib.sha256(Path(b).read_bytes()).hexdigest()} for n, b in servers},
            "harness_sha256": hashlib.sha256((a.bench_dir / "run_bench.py").read_bytes()).hexdigest()}
    runner_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    if a.resume_dir:
        old = json.loads((root / "metadata.json").read_text())
        for key in ["repetitions", "timed_seconds", "read_warmup_seconds", "server_cpus", "client_cpus", "harness_sha256", "soak_seconds", "soak_repetitions"]:
            if old[key] != meta[key]:
                ap.error(f"resume settings changed: {key}")
        for name, value in meta["binaries"].items():
            if value["sha256"] != old["binaries"][name]["sha256"]:
                ap.error(f"resume binary changed: {name}")
        meta["runner_revisions"] = old.get("runner_revisions", []) + [runner_hash]
    else:
        meta["runner_revisions"] = [runner_hash]
        for ds, n, pdfs, images in [("small", 200, 4, 8), ("large", 10000, 40, 80)]:
            subprocess.run([sys.executable, "-I", str(a.bench_dir / "gen/genvault.py"), "--out", f"{rb.DATA}/{ds}/vault",
                            "--index", f"{rb.DATA}/{ds}", "--notes", str(n), "--pdfs", str(pdfs), "--images", str(images)], check=True)
    record(root / "metadata.json", meta)
    plans = []
    for scenario in ["conflict", "rmw", "writes", "reads", "sse", "consistency"]:
        ds, runs = rb.SCENARIOS[scenario]
        for label, args in runs:
            if label in ["conflict-200", "rmw-200x5", "sse-500"]:
                continue
            args = [f"{a.seconds}s" if x == "10s" else x for x in args]
            if scenario == "reads":
                args += ["--warmup", "1s"]
            plans.append((scenario, ds, label, args))
    plans += [("reads", "large", "read-note-1", ["reads", "--kind", "note", "--index", "{idx}/notes.txt", "--clients", "1", "--duration", f"{a.seconds}s", "--warmup", "1s"]),
              ("reads", "large", "read-tree-1", ["reads", "--kind", "tree", "--clients", "1", "--duration", f"{a.seconds}s", "--warmup", "1s"]),
              ("writes", "small", "writes-1", ["writes", "--clients", "1", "--duration", f"{a.seconds}s", "--size", "4096"])]
    rows = json.loads((root / "raw.json").read_text()) if a.resume_dir else []
    completed = {(r["name"], r["label"], r["rep"]) for r in rows}
    consistency_failures = []
    for row in rows:
        if row.get("scenario") == "consistency":
            failed = [c for c in row["checks"] if not c["ok"]]
            if failed:
                consistency_failures.append({"name": row["name"], "rep": row["rep"], "checks": failed})
        else:
            assert_correct(row)
    for rep in range(a.reps):
        order = servers if rep % 2 == 0 else list(reversed(servers))
        for ds in ["small", "large"]:
            for name, binary in order:
                if (name, f"startup-{ds}", rep) in completed:
                    continue
                row = rb.startup_only(name, binary, ds, rep)
                row.update(name=name, label=f"startup-{ds}", rep=rep)
                rows.append(row)
                record(root / "raw.json", rows)
        for scenario, ds, label, args in plans:
            for name, binary in order:
                if (name, label, rep) in completed:
                    continue
                row = rb.run_one(name, binary, scenario, label, args, ds, rep)
                row.update(name=name, label=label, rep=rep)
                rows.append(row)
                record(root / "raw.json", rows)
                try:
                    assert_correct(row)
                except RuntimeError:
                    if row.get("scenario") != "consistency":
                        raise
                    failed = [c for c in row["checks"] if not c["ok"]]
                    consistency_failures.append({"name": name, "rep": rep, "checks": failed})
                    print(f"FAIL {name} consistency rep={rep+1}: {failed}", flush=True)
                record(root / "consistency-failures.json", consistency_failures)
                print(f"rep={rep+1} {name} {label}: rps={row.get('rps', '-')} lost={row.get('lost_updates', '-')} peak={row['server']['vmhwm_kb']/1024:.1f} MiB", flush=True)
    soaks = json.loads((root / "soak.json").read_text()) if a.resume_dir and (root / "soak.json").exists() else []
    for rep in range(a.soak_reps):
        for name, binary in (servers if rep % 2 == 0 else list(reversed(servers))):
            if any(r["name"] == name and r["rep"] == rep for r in soaks):
                continue
            row = soak(rb, binary, name, rep, a.soak_seconds)
            soaks.append(row)
            record(root / "soak.json", soaks)
            print(f"mixed rep={rep+1} {name}: peak={row['peak_rss_kib']/1024:.1f} MiB", flush=True)
    print(f"Complete: {len(rows)} runs, {len(soaks)} mixed loads; {root}", flush=True)
    record(root / "status.json", {"measurement_complete": True, "consistency_failures": consistency_failures})
    # Finish independent timing/memory measurements, preserve every failed
    # invariant, and still fail the correctness gate at the end of the campaign.
    if consistency_failures:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
