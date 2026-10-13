#!/usr/bin/env python3
"""Reproduction: lost updates under concurrent read-modify-write saves of ONE note.

W workers repeat: GET the note (content + mtime), append a unique line, PUT it back with the
mtime they read; a 200 is an accepted write (a 409 means "stale", the worker re-reads and retries).
If the optimistic-concurrency check were airtight, every accepted write would survive: the final
note has exactly W*N lines. A shortfall = a lost update (an accepted save overwritten by another
save that was based on an older version).

    lost_update.py --url http://127.0.0.1:9110 --vault /tmp/qbench/xxx/vault [--workers 8 --writes 15 --rounds 20]
Python standard library only. Prints one line per round and a summary.
"""
import argparse
import concurrent.futures as cf
import http.client
import json
import os
import threading
import urllib.parse


def call(host, port, method, path, obj=None):
    c = http.client.HTTPConnection(host, port, timeout=30)
    body = json.dumps(obj).encode() if obj is not None else None
    hdrs = {"Content-Type": "application/json"} if body else {}
    c.request(method, path, body=body, headers=hdrs)
    r = c.getresponse()
    data = r.read()
    c.close()
    return r.status, json.loads(data) if data else None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", required=True)
    ap.add_argument("--vault", required=True)
    ap.add_argument("--workers", type=int, default=8)
    ap.add_argument("--writes", type=int, default=15)
    ap.add_argument("--rounds", type=int, default=20)
    a = ap.parse_args()
    u = urllib.parse.urlparse(a.url)
    host, port = u.hostname, u.port
    lost_total = 0
    rounds_with_loss = 0
    for rnd in range(a.rounds):
        name = f"lostupdate-{rnd}.md"
        call(host, port, "PUT", f"/api/notes/{name}", {"content": ""})
        accepted = [0]
        lock = threading.Lock()

        def worker(w):
            done = 0
            while done < a.writes:
                st, cur = call(host, port, "GET", f"/api/notes/{name}")
                st, _ = call(host, port, "PUT", f"/api/notes/{name}", {"content": cur["content"] + f"{w}:{done}\n", "mtime": cur["mtime"]})
                if st == 200:
                    done += 1
                    with lock:
                        accepted[0] += 1
        with cf.ThreadPoolExecutor(a.workers) as ex:
            list(ex.map(worker, range(a.workers)))
        lines = [l for l in open(os.path.join(a.vault, name)).read().split("\n") if l]
        lost = accepted[0] - len(lines)
        lost_total += lost
        rounds_with_loss += lost > 0
        print(f"round {rnd:2d}: accepted {accepted[0]}, lines on disk {len(lines)}, lost {lost}")
    print(f"SUMMARY: {rounds_with_loss}/{a.rounds} rounds lost updates; {lost_total} accepted writes lost in total")


if __name__ == "__main__":
    main()
