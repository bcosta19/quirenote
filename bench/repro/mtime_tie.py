#!/usr/bin/env python3
"""Reproduction: two different versions of a note can carry the SAME mtime, so a stale save is accepted.

The optimistic-concurrency check is `current mtime == mtime the client read`. The mtime comes from the
kernel's coarse clock (a jiffy, 1-4 ms) and every atomic save is a NEW inode (temp file + rename), so two
saves within one tick get identical mtimes. Sequence per round, all from one client (no threads needed):

    PUT v1 (forced) -> m1 ;  PUT v2 with base m1 -> m2 ;  PUT v3 with the STALE base m1

v3 must be rejected (409): v2 replaced the version m1 describes. If m2 == m1 the server cannot tell,
accepts v3 and v2 is silently lost.

    mtime_tie.py --url http://127.0.0.1:9110 [--rounds 3000]
Prints how often m2 == m1 happened and how many stale saves were wrongly accepted. Standard library only.
"""
import argparse
import http.client
import json
import urllib.parse


def call(c, method, path, obj=None):
    body = json.dumps(obj).encode() if obj is not None else None
    c.request(method, path, body=body, headers={"Content-Type": "application/json"} if body else {})
    r = c.getresponse()
    data = r.read()
    return r.status, json.loads(data)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", required=True)
    ap.add_argument("--rounds", type=int, default=3000)
    a = ap.parse_args()
    u = urllib.parse.urlparse(a.url)
    c = http.client.HTTPConnection(u.hostname, u.port, timeout=30)
    ties = wrong = 0
    for i in range(a.rounds):
        p = f"/api/notes/mtime-tie/n{i % 50}.md"
        _, n1 = call(c, "PUT", p, {"content": f"v1 {i}"})
        m1 = n1["mtime"]
        st2, n2 = call(c, "PUT", p, {"content": f"v2 {i}", "mtime": m1})
        assert st2 == 200, (st2, n2)
        st3, _ = call(c, "PUT", p, {"content": f"v3 {i}", "mtime": m1})
        if n2["mtime"] == m1:
            ties += 1
        if st3 == 200:
            wrong += 1
    print(f"{a.rounds} rounds: v1 and v2 had the same mtime in {ties}; the stale save of v3 (base = v1's mtime) was ACCEPTED in {wrong} (should be 0)")


if __name__ == "__main__":
    main()
