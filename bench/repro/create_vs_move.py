#!/usr/bin/env python3
"""Reproduction: `POST /api/move` can silently overwrite a note that `POST /api/notes/...` just created.

Go's vault.Move does `Lstat(to)` (must not exist) and then `Rename(from, to)`; `Create` does not take
the vault mutex, so a note created between the two calls is replaced by the move: both requests
answer success and one file is gone. (The Go source calls it a "known limitation": os.Root has no
RENAME_NOREPLACE.)  The Rust port uses renameat2(RENAME_NOREPLACE) and never loses a file.

    create_vs_move.py --url http://127.0.0.1:9110 [--rounds 200]
Standard library only.
"""
import argparse
import concurrent.futures as cf
import http.client
import json
import threading
import urllib.parse


def post(host, port, path, obj):
    c = http.client.HTTPConnection(host, port, timeout=30)
    c.request("POST", path, body=json.dumps(obj).encode(), headers={"Content-Type": "application/json"})
    r = c.getresponse()
    r.read()
    c.close()
    return r.status


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", required=True)
    ap.add_argument("--rounds", type=int, default=200)
    a = ap.parse_args()
    u = urllib.parse.urlparse(a.url)
    host, port = u.hostname, u.port
    both = 0
    for r in range(a.rounds):
        post(host, port, f"/api/notes/cvm-repro/src{r}.md", {"content": "moved"})
        gate = threading.Barrier(2)

        def create():
            gate.wait()
            return post(host, port, f"/api/notes/cvm-repro/dest{r}.md", {"content": "created"})

        def move():
            gate.wait()
            return post(host, port, "/api/move", {"from": f"cvm-repro/src{r}.md", "to": f"cvm-repro/dest{r}.md"})
        with cf.ThreadPoolExecutor(2) as ex:
            fc, fm = ex.submit(create), ex.submit(move)
            sc, sm = fc.result(), fm.result()
        if sc == 201 and sm == 200:
            both += 1
    print(f"{both}/{a.rounds} rounds: create answered 201 AND move answered 200 (one of the two files was silently destroyed)")


if __name__ == "__main__":
    main()
