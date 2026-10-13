#!/usr/bin/env python3
"""Black-box HTTP contract suite for a Quirenote backend (Go or Rust).

It only talks HTTP to --url; --vault is the directory the server serves (used to seed it, to
modify files "from outside" like Neovim would, and to check the disk after a request).

    contract.py seed  VAULT_DIR
    contract.py run   --url http://127.0.0.1:9100 --vault VAULT_DIR --out result.json
    contract.py setup --url http://127.0.0.1:9100 --home HOME_DIR --config-dir XDG_DIR --out result.json
    contract.py diff  go.json rs.json

`run`/`setup` record, for every case, the status, the contract-relevant headers and the
(normalised) body. `diff` compares two recordings: any difference is a contract divergence unless
it is listed in KNOWN_DIVERGENCES (each with the reason). Invariant checks (counts, "no lost
update", ...) are asserted inside the cases and reported separately as CHECK lines.
Python standard library only.
"""
import argparse
import base64
import concurrent.futures as cf
import http.client
import json
import os
import re
import socket
import struct
import sys
import threading
import time
import urllib.parse
import zlib

# ----------------------------------------------------------------------------------------
# HTTP helpers
# ----------------------------------------------------------------------------------------

JSON = {"Content-Type": "application/json"}
KEY_HEADERS = [
    "content-type", "allow", "location", "content-security-policy", "x-content-type-options",
    "accept-ranges", "content-range", "cache-control",
]


class Resp:
    def __init__(self, status, headers, body):
        self.status, self.headers, self.body = status, headers, body

    def json(self):
        return json.loads(self.body)

    def text(self):
        return self.body.decode("utf-8", "replace")


class Client:
    def __init__(self, url):
        u = urllib.parse.urlparse(url)
        self.host, self.port = u.hostname, u.port

    def req(self, method, path, body=None, headers=None, timeout=30):
        c = http.client.HTTPConnection(self.host, self.port, timeout=timeout)
        try:
            path = path.replace(" ", "%20")
            c.putrequest(method, path, skip_accept_encoding=True)
            hdrs = dict(headers or {})
            if isinstance(body, (dict, list)):
                body = json.dumps(body).encode()
                hdrs.setdefault("Content-Type", "application/json")
            elif isinstance(body, str):
                body = body.encode()
            if body is not None:
                hdrs["Content-Length"] = str(len(body))
            for k, v in hdrs.items():
                c.putheader(k, v)
            c.endheaders(body)
            r = c.getresponse()
            data = r.read()
            return Resp(r.status, {k.lower(): v for k, v in r.getheaders()}, data)
        finally:
            c.close()

    def get(self, path, **kw):
        return self.req("GET", path, **kw)

    def j(self, method, path, obj, **kw):
        return self.req(method, path, body=obj, headers=dict(JSON, **kw.pop("headers", {})), **kw)

    def raw(self, data: bytes, read_timeout=3.0) -> bytes:
        """Sends raw bytes and returns whatever comes back (for malformed requests)."""
        s = socket.create_connection((self.host, self.port), timeout=read_timeout)
        s.sendall(data)
        out = b""
        try:
            while True:
                chunk = s.recv(65536)
                if not chunk:
                    break
                out += chunk
        except socket.timeout:
            pass
        s.close()
        return out


# ----------------------------------------------------------------------------------------
# Recording
# ----------------------------------------------------------------------------------------

STAMP = re.compile(r"\d{4}-\d{2}-\d{2} \d{6}\.\d{3}")
PASTED = re.compile(r"Pasted image \d{14}( \d+)?")
TMPNAME = re.compile(r"\.[^ \"]*\.quirenote-[0-9a-f]{8}\.tmp")
CHECKFILE = re.compile(r"\.quirenote-check-\d+")
MTIME = re.compile(r'"mtime":"\d+"')
DATE_HDR = re.compile(r"\b(Mon|Tue|Wed|Thu|Fri|Sat|Sun), \d\d \w{3} \d{4} \d\d:\d\d:\d\d GMT")


class Recorder:
    def __init__(self, vault=None):
        self.cases = {}
        self.checks = []
        self.vault = vault
        self.order = []

    def norm(self, text):
        text = STAMP.sub("<stamp>", text)
        text = PASTED.sub("Pasted image <ts>", text)
        text = MTIME.sub('"mtime":"<mtime>"', text)
        text = TMPNAME.sub("<tmp>", text)
        text = CHECKFILE.sub(".quirenote-check-<n>", text)
        if self.vault:
            text = text.replace(self.vault, "<VAULT>")
            text = text.replace(os.path.dirname(self.vault), "<PARENT>")
        return text

    def rec(self, name, r, sort_json=None, body=True, headers=None, extra=None):
        hdrs = {}
        for h in (headers or KEY_HEADERS):
            if h in r.headers:
                hdrs[h] = self.norm(r.headers[h])
        if "last-modified" in r.headers:
            seed_lm = time.strftime("%a, %d %b %Y %H:%M:%S GMT", time.gmtime(SEED_MTIME))
            hdrs["last-modified"] = "seed" if r.headers["last-modified"] == seed_lm else "dynamic"
        entry = {"status": r.status, "headers": hdrs}
        if body:
            text = r.text()
            if sort_json:
                try:
                    obj = json.loads(text)
                    obj = sort_json(obj)
                    text = json.dumps(obj, ensure_ascii=False, separators=(",", ":"))
                except Exception:  # keep raw text if it is not the expected JSON
                    pass
            entry["body"] = self.norm(text)
        if extra is not None:
            entry["extra"] = extra
        while name in self.cases:
            name += "'"
        self.cases[name] = entry
        self.order.append(name)
        return r

    def check(self, name, ok, detail=""):
        self.checks.append({"name": name, "ok": bool(ok), "detail": detail})
        print(("CHECK  ok   " if ok else "CHECK  FAIL ") + name + (f"  [{detail}]" if detail and not ok else ""))
        return ok


def by_path_line(res):
    return sorted(res, key=lambda r: (r.get("path", ""), r.get("line", 0)))


def cap_aware(res):
    """Results truncated at the cap depend on rg's (parallel, unordered) traversal."""
    if len(res) > 200:
        return {"capped_at": len(res), "all_hits_are_matches": all(any(p.get("hit") for p in r["parts"]) for r in res)}
    return by_path_line(res)


def sort_files_plan(plan):
    plan["files"] = sorted(plan["files"])
    return plan


def sort_relink(res):
    res["updated"] = sorted(res["updated"])
    res["skipped"] = sorted(res["skipped"])
    return res


# ----------------------------------------------------------------------------------------
# Seed
# ----------------------------------------------------------------------------------------

def tiny_png():
    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(b"\x00\x00\x00\x00\x00")) + chunk(b"IEND", b""))


SEED_MTIME = 1_700_000_000  # fixed so Last-Modified is comparable


def seed(vault):
    files = {
        "Welcome.md": "# Welcome\nSee [[Lexer]] and [[Estudos/Compiladores]].\n",
        "Estudos/Compiladores.md": "# Compiladores\n\nA FUNÇÃO do lexer é gerar tokens.\nLink [[Lexer#Regras|regras]]\n",
        "Estudos/Lexer.md": "# Lexer\nRegras\n",
        "Livros/Dragon.pdf": b"%PDF-1.4\n%fake pdf for the contract suite\n" + bytes(range(256)) * 4,
        "Livros/pic.png": tiny_png(),
        "Livros/book.epub": b"PK\x03\x04" + b"epub" * 100,
        "Livros/photo.jpg": b"\xff\xd8\xff\xe0" + b"jpeg" * 50,
        "notes/utf8.md": "Olá, mundo! <b>&amp;</b> ☃ \u2028 end\n",
        "notes/latin1.md": b"fun\xe7\xe3o em latin1\n",
        "notes/crlf.md": "line1\r\nline2\r\n",
        "notes/dots.v2/x.md": "inside a folder with a dot\n",
        "notes/plain.txt": "just text\n",
        "notes/unknown.xyz": bytes(range(256)),
        "notes/textual.zzz": "plain text with an unknown extension\n",
        "empty.md": "",
        "UPPER.MD": "função em arquivo MD maiúsculo\n",
        "ignored.md": "função listada no gitignore\n",
        ".gitignore": "ignored.md\n",
        ".hidden.md": "hidden note\n",
        ".obsidian/app.json": "{}",
        ".obsidian/workspace.json": "{}",
        "linked/A.md": "[[Target]] and [[linked/Target|alias]] and [[Target.md]] and [[Target#sec]]\n",
        "linked/Target.md": "I am the target [[Target]]\n",
        "linked/Table.md": "| a |\n|---|\n| [[Target\\|x]] |\n",
        "linked/Other.md": "[[Target extra]] and [[Outro]] and Target without link\n",
        "many/m0.md": "x\n",
    }
    for i in range(260):
        files[f"many/n{i:03d}.md"] = "hoje: needle line [[Needle]]\n"
    files["perigo.md"] = "$(rm -rf /) ; `x` [a-z]+ .*\n"
    files["longa.md"] = ("palavra" * 80) + "\n"
    files["multi.md"] = "\n".join(f"alpha line {i}" for i in range(12)) + "\n"
    for rel, body in files.items():
        p = os.path.join(vault, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.write(body if isinstance(body, bytes) else body.encode())
        os.utime(p, (SEED_MTIME, SEED_MTIME))
    os.chmod(os.path.join(vault, "notes/plain.txt"), 0o640)


# ----------------------------------------------------------------------------------------
# Cases
# ----------------------------------------------------------------------------------------

def tree_paths(entries):
    out = []
    for e in entries:
        out.append(e["path"])
        out.extend(tree_paths(e.get("children", [])))
    return out


def run_cases(c: Client, R: Recorder, vault: str, only=None):
    def section(name, fn):
        if only and name not in only:
            return
        print(f"--- {name}")
        try:
            fn()
        except Exception as e:  # a crash in a section must not hide the others
            import traceback
            traceback.print_exc()
            R.check(f"section {name} completed", False, repr(e))

    # ---- routing, redirects, methods, CSRF ------------------------------------------------
    def routing():
        R.rec("tree", c.get("/api/tree"))
        R.rec("tree HEAD", c.req("HEAD", "/api/tree"), body=False)
        R.rec("tree trailing slash -> SPA", c.get("/api/tree/"), body=False)
        R.rec("unknown api GET -> SPA", c.get("/api/nope"), body=False)
        for i, ct in enumerate([None, "text/plain", "application/x-www-form-urlencoded", "Application/JSON", "application/json;charset=utf-8", " application/json ; x=y", "application/jsonx"]):
            hdrs = {"Content-Type": ct} if ct else {}
            R.rec(f"POST /api/dirs ct={ct!r}", c.req("POST", f"/api/dirs/csrf-{i}", body="{}", headers=hdrs))
        for m in ["POST", "PUT", "DELETE", "PATCH"]:
            R.rec(f"{m} unknown path no content-type", c.req(m, "/nonexistent", body="{}"))
            R.rec(f"{m} unknown path json", c.req(m, "/nonexistent", body="{}", headers=JSON))
        for m, p in [("DELETE", "/api/tree"), ("PUT", "/api/move"), ("GET", "/api/move"), ("DELETE", "/api/notes/a.md"),
                     ("PUT", "/api/relink"), ("DELETE", "/api/files/a.md"), ("GET", "/api/dirs/x"), ("DELETE", "/api/setup"),
                     ("PUT", "/api/events"), ("POST", "/api/search"), ("POST", "/api/backlinks"), ("PUT", "/api/trash"),
                     ("GET", "/api/trash"), ("GET", "/api/images"), ("DELETE", "/api/notes/"), ("DELETE", "/api/files/")]:
            R.rec(f"{m} {p}", c.req(m, p, body="{}", headers=JSON), body=True)
        for p in ["/api/notes", "/api/files", "/api/dirs", "/api//tree", "/api/notes/x/../Welcome.md", "/api/./tree",
                  "/api/notes/Welcome.md/../Welcome.md?x=1&y=2", "//api/tree", "/api/notes/a%20b//c.md"]:
            R.rec(f"redirect GET {p}", c.get(p))
        R.rec("redirect POST /api/notes", c.j("POST", "/api/notes", {"content": "x"}))
        R.rec("redirect PUT /api/notes", c.j("PUT", "/api/notes", {"content": "x"}))
        R.rec("redirect POST /api/dirs", c.j("POST", "/api/dirs", {}))
        R.rec("redirect DELETE /api/notes", c.j("DELETE", "/api/notes", {}))
        R.rec("redirect HEAD /api/notes", c.req("HEAD", "/api/notes"))

    section("routing", routing)

    # ---- paths and path traversal ----------------------------------------------------------
    def paths():
        for p in ["Welcome.md", "/Welcome.md", "%2FWelcome.md", "welcome.md", "Estudos/../Welcome.md", "Estudos/%2e%2e/Welcome.md",
                  "%2e%2e/x.md", "..%2fx.md", "%2e%2e%2fetc%2fpasswd", "%2fetc%2fpasswd", "Estudos%2f..%2f..%2fx.md",
                  ".hidden.md", ".obsidian/app.json", "Estudos/.x.md", "missing.md", "Estudos", "Estudos/", "", "notes/plain.txt",
                  "notes/dots.v2/x.md", "..", "%2e%2e", "Welcome.md/", "a%00b.md", "Welcome.md%00", "%ff%fe.md",
                  "caf%C3%A9.md", "Estudos//Lexer.md", "./Welcome.md", "notes/latin1.md", "notes/utf8.md", "notes/crlf.md", "empty.md"]:
            R.rec(f"read {p!r}", c.get("/api/notes/" + p))
        for p in ["../etc/passwd", "..%2f..%2fetc%2fpasswd", "%2e%2e/%2e%2e/etc/passwd", ".obsidian/app.json", ".hidden.md"]:
            R.rec(f"files traversal {p!r}", c.get("/api/files/" + p))
            R.rec(f"save traversal {p!r}", c.j("PUT", "/api/notes/" + p, {"content": "pwned"}))
            R.rec(f"create traversal {p!r}", c.j("POST", "/api/notes/" + p, {"content": "pwned"}))
            R.rec(f"mkdir traversal {p!r}", c.j("POST", "/api/dirs/" + p, {}))
        # raw (un-normalised) request lines, as an attacker would send them
        for line in ["GET /api/notes/../../../../etc/passwd HTTP/1.1", "GET /api/files/../../etc/passwd HTTP/1.1",
                     "GET /api/files/%2e%2e/%2e%2e/etc/passwd HTTP/1.1", "GET /../../etc/passwd HTTP/1.1"]:
            raw = c.raw((line + "\r\nHost: x\r\nConnection: close\r\n\r\n").encode())
            head = raw.split(b"\r\n", 1)[0].decode(errors="replace")
            R.rec(f"raw {line}", Resp(int(head.split()[1]) if head.startswith("HTTP/") else 0, {}, b""), body=False,
                  extra={"leaks_passwd": b"root:" in raw})
        R.check("no traversal leaked /etc/passwd", all("leaks_passwd" not in v or not v["extra"]["leaks_passwd"] for v in R.cases.values() if "extra" in v and isinstance(v["extra"], dict)))
        outside = os.path.join(os.path.dirname(vault), "outside-secret.md")
        with open(outside, "w") as f:
            f.write("secret outside the vault")
        try:
            os.symlink(outside, os.path.join(vault, "link-out.md"))
            os.symlink("Welcome.md", os.path.join(vault, "link-in.md"))
            os.symlink(os.path.dirname(vault), os.path.join(vault, "dir-out"))
        except FileExistsError:
            pass
        r = R.rec("symlink escape read", c.get("/api/notes/link-out.md"))
        R.check("symlink escape not served", b"secret outside" not in r.body)
        r = R.rec("symlink escape files", c.get("/api/files/link-out.md"))
        R.check("symlink escape (files) not served", b"secret outside" not in r.body)
        R.rec("symlink escape via dir", c.get("/api/notes/dir-out/outside-secret.md"))
        R.rec("symlink inside vault read", c.get("/api/notes/link-in.md"))
        r = R.rec("symlink escape save", c.j("PUT", "/api/notes/link-out.md", {"content": "overwritten"}))
        R.check("outside file untouched by save through symlink", open(outside).read() == "secret outside the vault")
        R.rec("symlink escape mkdir", c.j("POST", "/api/dirs/dir-out/newdir", {}))
        R.check("no dir created outside the vault", not os.path.exists(os.path.join(os.path.dirname(vault), "newdir")))
        R.rec("tree with symlinks", c.get("/api/tree"))
        for n in ["link-out.md", "link-in.md", "dir-out"]:
            try:
                os.unlink(os.path.join(vault, n))
            except FileNotFoundError:
                pass

    section("paths", paths)

    # ---- notes: create / save / conflicts --------------------------------------------------
    def notes():
        r = R.rec("create", c.j("POST", "/api/notes/new/Idea.md", {"content": "# Idea\n"}))
        R.rec("create again 409", c.j("POST", "/api/notes/new/Idea.md", {"content": "other"}))
        R.check("create did not overwrite", c.get("/api/notes/new/Idea.md").json()["content"] == "# Idea\n")
        R.rec("create no content", c.j("POST", "/api/notes/new/Empty.md", {}))
        R.rec("create non-md", c.j("POST", "/api/notes/new/x.pdf", {"content": "x"}))
        R.rec("create uppercase .MD", c.j("POST", "/api/notes/new/Up.MD", {"content": "x"}))
        for bad in ["C%23%20notas.md", "x%5Ey.md", "a%20%5Bb%5D.md", "a%7Cb.md", "a%0Ab.md"]:
            R.rec(f"create bad name {bad}", c.j("POST", "/api/notes/new/" + bad, {"content": "x"}))
        R.rec("create invalid json", c.req("POST", "/api/notes/new/J.md", body="{nope", headers=JSON))
        R.rec("create empty body", c.req("POST", "/api/notes/new/J.md", body="", headers=JSON))
        R.rec("create json null", c.req("POST", "/api/notes/new/Null.md", body="null", headers=JSON))
        R.rec("create wrong type", c.req("POST", "/api/notes/new/W.md", body='{"content":5}', headers=JSON))
        R.rec("create unknown fields", c.req("POST", "/api/notes/new/U.md", body='{"content":"u","extra":1}', headers=JSON))
        R.rec("create trailing garbage", c.req("POST", "/api/notes/new/T.md", body='{"content":"t"} garbage', headers=JSON))
        R.rec("create capitalised key", c.req("POST", "/api/notes/new/K.md", body='{"Content":"k"}', headers=JSON))
        R.rec("create special chars", c.j("POST", "/api/notes/new/Special.md", {"content": "<script>&\u2028\u2029\u0000\u001f\"\\ é ☃ \U0001F600"}))
        R.rec("read special chars", c.get("/api/notes/new/Special.md"))
        R.rec("create big body 10MB limit", c.req("POST", "/api/notes/new/Big.md", body=json.dumps({"content": "x" * (10 << 20)}), headers=JSON))
        R.rec("create under 10MB", c.req("POST", "/api/notes/new/Big2.md", body=json.dumps({"content": "x" * (9 << 20)}), headers=JSON), body=False)
        R.rec("create through a file", c.j("POST", "/api/notes/Welcome.md/x.md", {"content": "x"}))
        R.rec("read utf8", c.get("/api/notes/notes/utf8.md"))

        base = c.get("/api/notes/Welcome.md").json()
        R.check("mtime is a decimal string", isinstance(base["mtime"], str) and base["mtime"].isdigit())
        st = os.stat(os.path.join(vault, "Welcome.md"))
        R.check("mtime equals st_mtime_ns", base["mtime"] == str(st.st_mtime_ns), f"{base['mtime']} vs {st.st_mtime_ns}")
        R.rec("save ok", c.j("PUT", "/api/notes/Welcome.md", {"content": "# Welcome v2\n", "mtime": base["mtime"]}))
        r = R.rec("save stale -> 409 with current", c.j("PUT", "/api/notes/Welcome.md", {"content": "stale", "mtime": base["mtime"]}))
        R.check("409 carries the current version", r.status == 409 and r.json()["content"] == "# Welcome v2\n")
        R.check("stale save did not write", open(os.path.join(vault, "Welcome.md")).read() == "# Welcome v2\n")
        R.rec("save force (no mtime)", c.j("PUT", "/api/notes/Welcome.md", {"content": "forced"}))
        R.rec("save force (mtime 0 string)", c.j("PUT", "/api/notes/Welcome.md", {"content": "forced0", "mtime": "0"}))
        R.rec("save new file with mtime -> 404", c.j("PUT", "/api/notes/ghost.md", {"content": "x", "mtime": "5"}))
        R.check("ghost not created", not os.path.exists(os.path.join(vault, "ghost.md")))
        R.rec("save new file force", c.j("PUT", "/api/notes/deep/er/new.md", {"content": "n"}))
        R.rec("save mtime as number -> 400", c.req("PUT", "/api/notes/Welcome.md", body='{"content":"x","mtime":5}', headers=JSON))
        R.rec("save mtime garbage -> 400", c.req("PUT", "/api/notes/Welcome.md", body='{"content":"x","mtime":"abc"}', headers=JSON))
        R.rec("save mtime empty string -> 400", c.req("PUT", "/api/notes/Welcome.md", body='{"content":"x","mtime":""}', headers=JSON))
        R.rec("save mtime null", c.req("PUT", "/api/notes/Welcome.md", body='{"content":"nullm","mtime":null}', headers=JSON))
        R.rec("save mtime negative", c.req("PUT", "/api/notes/Welcome.md", body='{"content":"x","mtime":"-1"}', headers=JSON))
        R.rec("save mtime overflow", c.req("PUT", "/api/notes/Welcome.md", body='{"content":"x","mtime":"99999999999999999999"}', headers=JSON))
        R.rec("save non-md", c.j("PUT", "/api/notes/Livros/Dragon.pdf", {"content": "x"}))
        R.rec("save upper .MD ext", c.j("PUT", "/api/notes/UPPER.MD", {"content": "upper saved"}))
        R.rec("save empty body", c.req("PUT", "/api/notes/Welcome.md", body="", headers=JSON))
        R.rec("save invalid json", c.req("PUT", "/api/notes/Welcome.md", body="[", headers=JSON))
        R.rec("save too big", c.req("PUT", "/api/notes/Welcome.md", body=json.dumps({"content": "x" * (10 << 20)}), headers=JSON))
        R.rec("save dir path", c.j("PUT", "/api/notes/Estudos.md", {"content": "x"}))
        R.rec("save over dir named .md", (os.makedirs(os.path.join(vault, "isdir.md"), exist_ok=True), c.j("PUT", "/api/notes/isdir.md", {"content": "x"}))[1])
        R.rec("read dir named .md", c.get("/api/notes/isdir.md"))
        R.rec("read dir", c.get("/api/notes/Estudos"))
        R.rec("read latin1", c.get("/api/notes/notes/latin1.md"))
        R.rec("read crlf", c.get("/api/notes/notes/crlf.md"))

        # permissions preserved, atomic write leaves nothing behind
        p = os.path.join(vault, "perm.md")
        open(p, "w").write("p")
        os.chmod(p, 0o600)
        cur = c.get("/api/notes/perm.md").json()
        c.j("PUT", "/api/notes/perm.md", {"content": "p2", "mtime": cur["mtime"]})
        R.check("save keeps 0600", (os.stat(p).st_mode & 0o777) == 0o600, oct(os.stat(p).st_mode & 0o777))
        c.j("PUT", "/api/notes/perm.md", {"content": "p3"})
        R.check("forced save keeps 0600", (os.stat(p).st_mode & 0o777) == 0o600)
        c.j("PUT", "/api/notes/permnew.md", {"content": "n"})
        R.check("new file is 0644 & umask", (os.stat(os.path.join(vault, "permnew.md")).st_mode & 0o777) in (0o644, 0o640, 0o600), oct(os.stat(os.path.join(vault, "permnew.md")).st_mode & 0o777))
        tmp = [n for _, _, fs in os.walk(vault) for n in fs if n.endswith(".tmp")]
        R.check("no temporary files left", not tmp, str(tmp))

        # external change (Neovim) -> stale
        note = c.get("/api/notes/Estudos/Lexer.md").json()
        lp = os.path.join(vault, "Estudos/Lexer.md")
        open(lp, "w").write("# changed outside\n")
        t = time.time() + 5
        os.utime(lp, (t, t))
        r = R.rec("save after external change", c.j("PUT", "/api/notes/Estudos/Lexer.md", {"content": "mine", "mtime": note["mtime"]}))
        R.check("external change returned as 409", r.status == 409 and r.json()["content"] == "# changed outside\n")
        open(lp, "w").write("# Lexer\nRegras\n")
        os.utime(lp, (SEED_MTIME, SEED_MTIME))

        # moved/trashed notes must not be resurrected by a stale autosave
        b = c.j("POST", "/api/notes/stale/draft.md", {"content": "original"}).json()
        c.j("POST", "/api/move", {"from": "stale/draft.md", "to": "stale-archive/draft.md"})
        R.rec("stale save after move", c.j("PUT", "/api/notes/stale/draft.md", {"content": "autosave", "mtime": b["mtime"]}))
        R.check("stale save did not recreate the old path", not os.path.exists(os.path.join(vault, "stale/draft.md")))
        b = c.j("POST", "/api/notes/stale/draft2.md", {"content": "original"}).json()
        c.j("POST", "/api/trash", {"path": "stale/draft2.md"})
        R.rec("stale save after trash", c.j("PUT", "/api/notes/stale/draft2.md", {"content": "autosave", "mtime": b["mtime"]}))
        R.check("stale save after trash did not recreate", not os.path.exists(os.path.join(vault, "stale/draft2.md")))

    section("notes", notes)

    # ---- dirs, move, trash -------------------------------------------------------------------
    def dirs_move_trash():
        R.rec("mkdir", c.j("POST", "/api/dirs/Estudos/Novo", {}))
        R.rec("mkdir again", c.j("POST", "/api/dirs/Estudos/Novo", {}))
        R.rec("mkdir leading slash", c.j("POST", "/api/dirs/%2Fetc/x", {}))
        R.check("mkdir /etc/x is inside the vault", os.path.isdir(os.path.join(vault, "etc/x")))
        for bad in [".oculta", "a/.obsidian", "Pasta%20%231", "a%7Cb"]:
            R.rec(f"mkdir bad {bad}", c.j("POST", "/api/dirs/" + bad, {}))
        R.rec("mkdir over a file", c.j("POST", "/api/dirs/Welcome.md/sub", {}))
        R.rec("mkdir ignores body", c.req("POST", "/api/dirs/nobody", body="not json at all", headers=JSON))

        c.j("POST", "/api/notes/mv/a.md", {"content": "A"})
        c.j("POST", "/api/notes/mv/b.md", {"content": "B"})
        R.rec("move", c.j("POST", "/api/move", {"from": "mv/a.md", "to": "mv2/deep/a.md"}))
        R.rec("move onto existing 409", c.j("POST", "/api/move", {"from": "mv/b.md", "to": "mv2/deep/a.md"}))
        R.check("move kept both", open(os.path.join(vault, "mv/b.md")).read() == "B" and open(os.path.join(vault, "mv2/deep/a.md")).read() == "A")
        R.rec("move same path", c.j("POST", "/api/move", {"from": "mv/b.md", "to": "mv/b.md"}))
        R.rec("move folder into itself", c.j("POST", "/api/move", {"from": "mv2", "to": "mv2/sub"}))
        R.rec("move folder", c.j("POST", "/api/move", {"from": "mv2", "to": "mv3"}))
        R.rec("move missing", c.j("POST", "/api/move", {"from": "nope.md", "to": "x.md"}))
        R.rec("move bad name", c.j("POST", "/api/move", {"from": "mv/b.md", "to": "b#.md"}))
        R.rec("move from outside", c.j("POST", "/api/move", {"from": "../x", "to": "y.md"}))
        R.rec("move to outside", c.j("POST", "/api/move", {"from": "mv/b.md", "to": "../y.md"}))
        R.rec("move hidden source", c.j("POST", "/api/move", {"from": ".obsidian/app.json", "to": "x.json"}))
        R.rec("move into trash", c.j("POST", "/api/move", {"from": "mv/b.md", "to": ".trash/b.md"}))
        R.rec("move missing fields", c.j("POST", "/api/move", {}))
        R.rec("move invalid json", c.req("POST", "/api/move", body="{", headers=JSON))
        R.rec("move case only", c.j("POST", "/api/move", {"from": "mv/b.md", "to": "mv/B.md"}))
        R.rec("move a pdf", c.j("POST", "/api/move", {"from": "Livros/book.epub", "to": "Livros/book2.epub"}))
        c.j("POST", "/api/move", {"from": "Livros/book2.epub", "to": "Livros/book.epub"})

        r1 = R.rec("trash", c.j("POST", "/api/trash", {"path": "mv/B.md"}))
        c.j("POST", "/api/notes/mv/B.md", {"content": "second"})
        r2 = R.rec("trash same name again", c.j("POST", "/api/trash", {"path": "mv/B.md"}))
        p1, p2 = r1.json().get("path"), r2.json().get("path")
        R.check("trash keeps both versions", p1 != p2 and os.path.exists(os.path.join(vault, p1)) and os.path.exists(os.path.join(vault, p2)), f"{p1} {p2}")
        R.check("trash names follow Go's scheme", p1 == ".trash/B.md" and re.fullmatch(r"\.trash/B \d{4}-\d\d-\d\d \d{6}\.\d{3}\.md", p2 or "") is not None, f"{p1} {p2}")
        R.rec("trash folder", c.j("POST", "/api/trash", {"path": "mv3"}))
        R.rec("trash the trash", c.j("POST", "/api/trash", {"path": ".trash"}))
        R.rec("trash inside the trash", c.j("POST", "/api/trash", {"path": ".trash/B.md"}))
        R.rec("trash missing", c.j("POST", "/api/trash", {"path": "gone.md"}))
        R.rec("trash outside", c.j("POST", "/api/trash", {"path": "../x"}))
        R.rec("trash empty path", c.j("POST", "/api/trash", {"path": ""}))
        R.rec("trash no fields", c.j("POST", "/api/trash", {}))
        # 40 same-named notes trashed in a row: every version survives
        seen = {}
        for i in range(40):
            c.j("POST", f"/api/notes/batch/{i}/note.md", {"content": f"version {i}"})
            p = c.j("POST", "/api/trash", {"path": f"batch/{i}/note.md"}).json()["path"]
            seen[p] = f"version {i}"
        R.check("40 trashed versions all distinct", len(seen) == 40)
        R.check("40 trashed versions all intact", all(open(os.path.join(vault, p)).read() == v for p, v in seen.items()))
        R.rec("tree has no .trash", c.get("/api/tree"), sort_json=lambda t: [e["path"] for e in t])

    section("dirs_move_trash", dirs_move_trash)

    # ---- images ------------------------------------------------------------------------------
    def images():
        png = base64.b64encode(tiny_png()).decode()
        R.rec("image ok", c.j("POST", "/api/images", {"note": "Estudos/Lexer.md", "data": png}))
        R.rec("image ok again same second", c.j("POST", "/api/images", {"note": "Estudos/Lexer.md", "data": png}))
        for name, data in [("svg", b"<svg onload=alert(1)></svg>"), ("text", b"# nota"), ("empty", b""), ("html", b"<html></html>")]:
            R.rec(f"image {name}", c.j("POST", "/api/images", {"note": "a.md", "data": base64.b64encode(data).decode()}))
        R.rec("image bad base64", c.j("POST", "/api/images", {"note": "a.md", "data": "%%%"}))
        R.rec("image no data", c.j("POST", "/api/images", {"note": "a.md"}))
        R.rec("image invalid json", c.req("POST", "/api/images", body="x", headers=JSON))
        for name, data in [("jpeg", b"\xff\xd8\xff\xe0" + b"x" * 20), ("gif", b"GIF89a" + b"x" * 20), ("webp", b"RIFF\x00\x00\x00\x00WEBPVP8 " + b"x" * 20),
                           ("bmp", b"BM" + b"\x00" * 52)]:
            R.rec(f"image {name}", c.j("POST", "/api/images", {"note": "a.md", "data": base64.b64encode(data).decode()}))
        big = base64.b64encode(b"\x89PNG\r\n\x1a\n" + b"\0" * (21 << 20)).decode()
        R.rec("image too large 413", c.j("POST", "/api/images", {"note": "a.md", "data": big}))
        cfg = os.path.join(vault, ".obsidian/app.json")
        for conf in ["./", "./anexos", "Anexos", "/", "../fora", "  Spaced  ", ".hiddenfolder", "a/../b"]:
            open(cfg, "w").write(json.dumps({"attachmentFolderPath": conf}))
            R.rec(f"image attachment folder {conf!r}", c.j("POST", "/api/images", {"note": "Livros/nota.md", "data": png}))
        open(cfg, "w").write("not json")
        R.rec("image with broken app.json", c.j("POST", "/api/images", {"note": "Livros/nota.md", "data": png}))
        open(cfg, "w").write("{}")
        R.rec("tree includes images and bmp", c.get("/api/tree"), sort_json=lambda t: sorted(tree_paths(t)))

    section("images", images)

    # ---- files ---------------------------------------------------------------------------------
    def files():
        for p in ["Welcome.md", "Livros/Dragon.pdf", "Livros/pic.png", "Livros/book.epub", "Livros/photo.jpg", "notes/plain.txt",
                  "notes/unknown.xyz", "notes/textual.zzz", "notes/utf8.md", "empty.md", "Estudos", "missing.pdf", ".hidden.md", "notes/dots.v2/x.md"]:
            R.rec(f"file {p}", c.get("/api/files/" + p))
        R.rec("file HEAD", c.req("HEAD", "/api/files/Livros/Dragon.pdf"))
        for rng in ["bytes=0-9", "bytes=10-", "bytes=-20", "bytes=0-0", "bytes=5000-", "bytes=1000-2000", "bytes=-5000", "bytes=2-1", "bytes=abc", "items=0-5",
                    "bytes=0-5,10-15", "bytes=", "bytes=-", "bytes=0-9 ", "bytes=--5", "bytes=0-1,"]:
            r = c.get("/api/files/Livros/Dragon.pdf", headers={"Range": rng})
            R.rec(f"range {rng!r}", r, body=True)
        lm = c.get("/api/files/Livros/Dragon.pdf").headers.get("last-modified")
        R.rec("if-modified-since equal -> 304", c.get("/api/files/Livros/Dragon.pdf", headers={"If-Modified-Since": lm}))
        R.rec("if-modified-since old -> 200", c.get("/api/files/Livros/Dragon.pdf", headers={"If-Modified-Since": "Mon, 01 Jan 2001 00:00:00 GMT"}), body=False)
        R.rec("if-range match", c.get("/api/files/Livros/Dragon.pdf", headers={"Range": "bytes=0-3", "If-Range": lm}))
        R.rec("if-range mismatch", c.get("/api/files/Livros/Dragon.pdf", headers={"Range": "bytes=0-3", "If-Range": "Mon, 01 Jan 2001 00:00:00 GMT"}), body=False)
        R.rec("range on HEAD", c.req("HEAD", "/api/files/Livros/Dragon.pdf", headers={"Range": "bytes=0-3"}))
        big = os.path.join(vault, "big.bin")
        open(big, "wb").write(os.urandom(3 << 20))
        r = c.get("/api/files/big.bin", headers={"Range": "bytes=1048576-2097151"})
        R.check("range of a 3MB file has the right length", r.status == 206 and len(r.body) == 1048576)
        R.check("range bytes match disk", r.body == open(big, "rb").read()[1048576:2097152])
        R.rec("big file range headers", r, body=False)
        os.unlink(big)

    section("files", files)

    # ---- search / backlinks / relink ---------------------------------------------------------------
    def searching():
        def S(q):
            return "/api/search?q=" + urllib.parse.quote(q)
        for q in ["funcao", "função", "FUNÇÃO", "needle", "[a-z]+ .*", "$(rm", "alpha", "arquivo MD", "nada-existe-aqui", "", "   ", " lexer ", "a+b", "(", "\\", "tokens", "regras", "palavra" * 40, "x" * 201, "x" * 200, "ol", "ol%", "é", "c", "n"]:
            R.rec(f"search {q[:30]!r}", c.get(S(q)), sort_json=cap_aware)
        R.rec("search duplicate q keeps first", c.get("/api/search?q=funcao&q=needle"), sort_json=by_path_line)
        R.rec("search plus is space", c.get("/api/search?q=lexer+%C3%A9"), sort_json=by_path_line)
        R.rec("search bad escape", c.get("/api/search?q=%zz"), sort_json=by_path_line)
        R.rec("search no q", c.get("/api/search"), sort_json=by_path_line)
        r = c.get(S("needle"))
        R.check("search returns at most 201 hits", len(r.json()) == 201, str(len(r.json())))
        r = c.get(S("alpha"))
        R.check("search returns at most 5 hits per file", len(r.json()) == 5, str(len(r.json())))
        R.rec("search latin1", c.get(S("funcao em")), sort_json=by_path_line)
        R.rec("search long line", c.get(S("palavra" * 40)), sort_json=by_path_line)
        for note in ["linked/Target.md", "/linked/Target.md", "Target.md", "Welcome.md", "Estudos/Lexer.md", "../x.md", "", "Livros/Dragon.pdf", "a.b.md", "linked/Table.md", "Needle.md", "x%00.md"]:
            R.rec(f"backlinks {note!r}", c.get("/api/backlinks?note=" + urllib.parse.quote(note)), sort_json=cap_aware)

        # relink
        c.j("POST", "/api/notes/rl/Old name.md", {"content": "# Old\n[[Old name]] self\n"})
        c.j("POST", "/api/notes/rl/ref1.md", {"content": "see [[Old name]] and [[rl/Old name#sec|alias]] and ![[old name.md]]\n"})
        c.j("POST", "/api/notes/rl/ref2.md", {"content": "| [[Old name\\|x]] |\n"})
        c.j("POST", "/api/notes/rl/ref3.md", {"content": "nothing\n"})
        R.rec("relink preview before rename -> 400", c.get("/api/relink?from=" + urllib.parse.quote("rl/Old name.md") + "&to=" + urllib.parse.quote("rl/New name.md")))
        c.j("POST", "/api/move", {"from": "rl/Old name.md", "to": "rl/New name.md"})
        q = "from=" + urllib.parse.quote("rl/Old name.md") + "&to=" + urllib.parse.quote("rl/New name.md")
        R.rec("relink preview", c.get("/api/relink?" + q), sort_json=sort_files_plan)
        R.rec("relink run", c.j("POST", "/api/relink", {"from": "rl/Old name.md", "to": "rl/New name.md"}), sort_json=sort_relink)
        R.rec("relink result ref1", c.get("/api/notes/rl/ref1.md"))
        R.rec("relink result ref2", c.get("/api/notes/rl/ref2.md"))
        R.rec("relink result self", c.get("/api/notes/rl/New name.md"))
        R.rec("relink run again (nothing left)", c.j("POST", "/api/relink", {"from": "rl/Old name.md", "to": "rl/New name.md"}), sort_json=sort_relink)
        # many referencing notes
        for i in range(60):
            c.j("POST", f"/api/notes/rl/many/{i:02d}.md", {"content": "hoje: [[Gone]]\n"})
        c.j("POST", "/api/notes/rl/Gone2.md", {"content": "x"})
        os.rename(os.path.join(vault, "rl/Gone2.md"), os.path.join(vault, "rl/Gone2.md"))
        c.j("POST", "/api/move", {"from": "rl/Gone2.md", "to": "rl/Renamed.md"})
        R.rec("relink with no referencing notes", c.j("POST", "/api/relink", {"from": "rl/Gone2.md", "to": "rl/Renamed.md"}), sort_json=sort_relink)
        # ambiguous: another file with the old name remains
        c.j("POST", "/api/notes/amb/a/Lexer.md", {"content": "x"})
        c.j("POST", "/api/notes/amb/b/Lexer.md", {"content": "y"})
        c.j("POST", "/api/notes/amb/byname.md", {"content": "[[Lexer]]\n"})
        c.j("POST", "/api/notes/amb/bypath.md", {"content": "[[amb/a/Lexer]]\n"})
        c.j("POST", "/api/move", {"from": "amb/a/Lexer.md", "to": "amb/a/Tokens.md"})
        q = "from=" + urllib.parse.quote("amb/a/Lexer.md") + "&to=" + urllib.parse.quote("amb/a/Tokens.md")
        R.rec("relink ambiguous preview", c.get("/api/relink?" + q), sort_json=sort_files_plan)
        R.rec("relink ambiguous run", c.j("POST", "/api/relink", {"from": "amb/a/Lexer.md", "to": "amb/a/Tokens.md"}), sort_json=sort_relink)
        R.rec("relink ambiguous byname", c.get("/api/notes/amb/byname.md"))
        R.rec("relink ambiguous bypath", c.get("/api/notes/amb/bypath.md"))
        # rejections
        R.rec("relink source still exists", c.j("POST", "/api/relink", {"from": "Welcome.md", "to": "empty.md"}))
        R.rec("relink dest missing", c.j("POST", "/api/relink", {"from": "sumiu.md", "to": "nao-existe.md"}))
        R.rec("relink type changed", c.j("POST", "/api/relink", {"from": "sumiu.md", "to": "Livros/Dragon.pdf"}))
        R.rec("relink bad name", c.j("POST", "/api/relink", {"from": "sumiu.md", "to": "Livros/pic#.png"}))
        R.rec("relink invalid json", c.req("POST", "/api/relink", body="{", headers=JSON))
        R.rec("relink preview no params", c.get("/api/relink"))
        R.rec("relink traversal", c.j("POST", "/api/relink", {"from": "../x.md", "to": "../y.md"}))
        # pdf rename
        c.j("POST", "/api/notes/pdfref.md", {"content": "[[Dragon.pdf#page=12]] and [[Livros/Dragon.pdf]]\n"})
        c.j("POST", "/api/move", {"from": "Livros/Dragon.pdf", "to": "Livros/Dragon Book.pdf"})
        R.rec("relink pdf", c.j("POST", "/api/relink", {"from": "Livros/Dragon.pdf", "to": "Livros/Dragon Book.pdf"}), sort_json=sort_relink)
        R.rec("relink pdf result", c.get("/api/notes/pdfref.md"))
        c.j("POST", "/api/move", {"from": "Livros/Dragon Book.pdf", "to": "Livros/Dragon.pdf"})
        # case only rename
        c.j("POST", "/api/notes/case/lexer.md", {"content": "x"})
        c.j("POST", "/api/notes/case/cita.md", {"content": "[[Lexer]]\n"})
        R.rec("relink case-only", c.j("POST", "/api/relink", {"from": "case/Lexer.md", "to": "case/lexer.md"}), sort_json=sort_relink)
        R.rec("relink case-only result", c.get("/api/notes/case/cita.md"))
        # a note whose name has a newline (imported)
        open(os.path.join(vault, "imported\nreference.md"), "w").write("[[Oldie]]\n")
        c.j("POST", "/api/notes/Newie.md", {"content": "x"})
        R.rec("relink with newline in referencing name", c.j("POST", "/api/relink", {"from": "Oldie.md", "to": "Newie.md"}), sort_json=sort_relink)
        os.unlink(os.path.join(vault, "imported\nreference.md"))

    section("search", searching)

    # ---- SPA and static ------------------------------------------------------------------------------
    def spa():
        R.rec("GET /", c.get("/"))
        R.rec("HEAD /", c.req("HEAD", "/"))
        R.rec("GET /index.html", c.get("/index.html"))
        R.rec("GET /index.html?x=1", c.get("/index.html?x=1"))
        R.rec("GET /some/route", c.get("/some/route"))
        R.rec("GET /deep/index.html", c.get("/deep/index.html"))
        R.rec("GET /%20weird", c.get("/%20weird"))
        idx = c.get("/").text()
        assets = sorted(set(re.findall(r'(?:src|href)="(/[^"]+\.(?:js|css|svg|png|webmanifest))"', idx)))
        for a in assets + ["/icon.svg", "/icon-512.png", "/manifest.webmanifest", "/theme-init.js", "/assets", "/assets/", "/.keep", "/favicon.ico", "/robots.txt"]:
            r = c.get(a)
            R.rec(f"static {a}", r, body=False, extra={"len": len(r.body), "sha": __import__("hashlib").sha256(r.body).hexdigest()[:16]})
        R.rec("static range", c.get("/theme-init.js", headers={"Range": "bytes=0-9"}))
        R.rec("static If-Modified-Since", c.get("/theme-init.js", headers={"If-Modified-Since": "Mon, 01 Jan 2030 00:00:00 GMT"}), body=False)
        R.rec("POST /", c.j("POST", "/", {}))
        R.rec("PUT /x", c.j("PUT", "/x", {}))
        R.rec("OPTIONS /api/tree", c.req("OPTIONS", "/api/tree"))
        R.rec("OPTIONS /", c.req("OPTIONS", "/"))

    section("spa", spa)

    # ---- raw HTTP oddities -----------------------------------------------------------------------------
    def rawhttp():
        def status(raw):
            head = raw.split(b"\r\n", 1)[0].decode(errors="replace")
            return int(head.split()[1]) if head.startswith("HTTP/") and len(head.split()) > 1 else 0
        cases = {
            "no host header http/1.1": b"GET /api/tree HTTP/1.1\r\nConnection: close\r\n\r\n",
            "http/1.0": b"GET /api/tree HTTP/1.0\r\n\r\n",
            "garbage": b"HELLO\r\n\r\n",
            "bad method": b"BREW /api/tree HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
            "chunked save": b"PUT /api/notes/chunked.md HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n11\r\n{\"content\":\"abc\"}\r\n0\r\n\r\n",
            "expect 100": b"PUT /api/notes/expect.md HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: 15\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n{\"content\":\"e\"}",
            "very long uri": b"GET /api/notes/" + b"a" * 70000 + b" HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
            "absolute-form": b"GET http://x/api/tree HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
        }
        for name, data in cases.items():
            raw = c.raw(data, read_timeout=2.0)
            R.rec(f"raw {name}", Resp(status(raw), {}, b""), body=False)
        R.check("chunked save worked", os.path.exists(os.path.join(vault, "chunked.md")) and open(os.path.join(vault, "chunked.md")).read() == "abc")

    section("rawhttp", rawhttp)

    # ---- SSE ---------------------------------------------------------------------------------------------
    def sse():
        def open_stream():
            s = socket.create_connection((c.host, c.port), timeout=5)
            s.sendall(b"GET /api/events HTTP/1.1\r\nHost: x\r\nAccept: text/event-stream\r\n\r\n")
            return s

        def read_until(s, pred, timeout):
            buf = b""
            end = time.time() + timeout
            s.settimeout(0.2)
            while time.time() < end:
                try:
                    d = s.recv(65536)
                    if not d:
                        break
                    buf += d
                except socket.timeout:
                    pass
                if pred(buf):
                    break
            return buf

        time.sleep(1.0)  # let the watcher flush events of the previous sections first
        s = open_stream()
        head = read_until(s, lambda b: b": connected" in b, 3)
        hdr = head.split(b"\r\n\r\n")[0].decode().lower()
        R.rec("sse headers", Resp(int(head.split()[1]), {k.strip(): v.strip() for k, v in (l.split(":", 1) for l in hdr.split("\r\n")[1:] if ":" in l)}, b""),
              headers=["content-type", "cache-control", "transfer-encoding"], body=False)
        R.check("sse sends ': connected' first", b": connected\n\n" in head)

        def events_for(action, settle=0.6):
            read_until(s, lambda b: False, 0.0)
            before = len(collected)
            action()
            end = time.time() + 2.5
            last = None
            buf = b""
            while time.time() < end:
                buf += read_until(s, lambda b: False, 0.25)
                evs = parse(buf)
                if evs and last is None:
                    last = time.time()
                if last and time.time() - last > settle:
                    break
            return sorted({(e["path"], e["op"]) for e in parse(buf)})

        def parse(buf):
            out = []
            for m in re.finditer(rb"data: (\{.*?\})\n\n", buf):
                out.append(json.loads(m.group(1)))
            return out

        collected = []
        time.sleep(0.3)

        def ext_write():
            open(os.path.join(vault, "sse1.md"), "w").write("one")
        R.rec("sse external create", Resp(200, {}, b""), body=False, extra=events_for(ext_write))

        def ext_append():
            open(os.path.join(vault, "sse1.md"), "a").write("two")
        R.rec("sse external append", Resp(200, {}, b""), body=False, extra=events_for(ext_append))

        def ext_rm():
            os.unlink(os.path.join(vault, "sse1.md"))
        R.rec("sse external remove", Resp(200, {}, b""), body=False, extra=events_for(ext_rm))

        def ext_mv():
            open(os.path.join(vault, "sse2.md"), "w").write("x")
            time.sleep(0.5)
            read_until(s, lambda b: False, 0.3)
            os.rename(os.path.join(vault, "sse2.md"), os.path.join(vault, "sse3.md"))
        evs = events_for(ext_mv, settle=0.8)
        R.rec("sse external rename", Resp(200, {}, b""), body=False, extra=[e for e in evs if e[0] in ("sse3.md",) or (e[0] == "sse2.md" and e[1] == "rename")])

        def api_save():
            c.j("PUT", "/api/notes/sse-api.md", {"content": "via api"})
        R.rec("sse api save", Resp(200, {}, b""), body=False, extra=events_for(api_save))

        def hidden():
            open(os.path.join(vault, ".obsidian/sse.json"), "w").write("{}")
            open(os.path.join(vault, ".hid-sse.md"), "w").write("{}")
        R.rec("sse hidden files are not reported", Resp(200, {}, b""), body=False, extra=events_for(hidden))

        def newdir():
            os.makedirs(os.path.join(vault, "ssedir"))
        R.rec("sse new dir", Resp(200, {}, b""), body=False, extra=events_for(newdir))
        time.sleep(0.4)
        read_until(s, lambda b: False, 0.3)

        def in_new_dir():
            open(os.path.join(vault, "ssedir/inside.md"), "w").write("x")
        R.rec("sse file inside new dir is watched", Resp(200, {}, b""), body=False, extra=events_for(in_new_dir))

        def burst():
            for i in range(5):
                open(os.path.join(vault, "burst.md"), "a").write(str(i))
                time.sleep(0.02)
        R.rec("sse burst coalesced", Resp(200, {}, b""), body=False, extra=events_for(burst))

        def api_trash():
            c.j("POST", "/api/trash", {"path": "burst.md"})
        R.rec("sse api trash", Resp(200, {}, b""), body=False, extra=events_for(api_trash))
        s.close()
        for n in ["sse3.md", "sse-api.md", ".hid-sse.md"]:
            try:
                os.unlink(os.path.join(vault, n))
            except FileNotFoundError:
                pass

    section("sse", sse)

    # ---- concurrency (correctness) --------------------------------------------------------------------------
    def concurrency():
        n = 64
        c.j("POST", "/api/notes/conc/same.md", {"content": "base"})
        base = c.get("/api/notes/conc/same.md").json()
        bodies = {i: f"client {i}\n" + "x" * 200_000 for i in range(n)}

        def one(i):
            cc = Client(f"http://{c.host}:{c.port}")
            return i, cc.j("PUT", "/api/notes/conc/same.md", {"content": bodies[i], "mtime": base["mtime"]}).status
        with cf.ThreadPoolExecutor(n) as ex:
            res = list(ex.map(one, range(n)))
        ok = [i for i, s in res if s == 200]
        R.check(f"{n} concurrent saves of one version: exactly one 200", len(ok) == 1, f"{len(ok)} accepted")
        R.check("others got 409", sum(1 for _, s in res if s == 409) == n - 1, str(sorted({s for _, s in res})))
        final = open(os.path.join(vault, "conc/same.md")).read()
        R.check("final content is exactly the winner's", len(ok) == 1 and final == bodies[ok[0]])
        R.check("no tmp left after concurrent saves", not [f for f in os.listdir(os.path.join(vault, "conc")) if f.endswith(".tmp")])

        # read-modify-write loop: no lost update
        c.j("PUT", "/api/notes/conc/rmw.md", {"content": ""})

        def worker(w):
            cc = Client(f"http://{c.host}:{c.port}")
            done = 0
            while done < 15:
                cur = cc.get("/api/notes/conc/rmw.md").json()
                r = cc.j("PUT", "/api/notes/conc/rmw.md", {"content": cur["content"] + f"{w}:{done}\n", "mtime": cur["mtime"]})
                if r.status == 200:
                    done += 1
            return w
        with cf.ThreadPoolExecutor(8) as ex:
            list(ex.map(worker, range(8)))
        lines = open(os.path.join(vault, "conc/rmw.md")).read().split("\n")
        lines = [l for l in lines if l]
        R.check("read-modify-write: 8 workers x 15 accepted writes = 120 lines (no lost update)", len(lines) == 120 and len(set(lines)) == 120, f"{len(lines)} lines, {len(set(lines))} unique")

        # concurrent create of the same path
        def create(i):
            cc = Client(f"http://{c.host}:{c.port}")
            return cc.j("POST", "/api/notes/conc/created.md", {"content": f"c{i}"}).status
        with cf.ThreadPoolExecutor(32) as ex:
            st = list(ex.map(create, range(32)))
        R.check("concurrent create: exactly one 201, the rest 409", st.count(201) == 1 and st.count(409) == 31, str({s: st.count(s) for s in set(st)}))
        R.check("created file is one writer's complete content", re.fullmatch(r"c\d+", open(os.path.join(vault, "conc/created.md")).read()) is not None)

        # concurrent move of different sources to the same destination
        for i in range(16):
            c.j("POST", f"/api/notes/conc/src{i}.md", {"content": f"s{i}"})

        def mv(i):
            cc = Client(f"http://{c.host}:{c.port}")
            return cc.j("POST", "/api/move", {"from": f"conc/src{i}.md", "to": "conc/dest.md"}).status
        with cf.ThreadPoolExecutor(16) as ex:
            st = list(ex.map(mv, range(16)))
        R.check("concurrent moves to one destination: exactly one 200, others 409", st.count(200) == 1 and st.count(409) == 15, str({s: st.count(s) for s in set(st)}))
        survivors = [f for f in os.listdir(os.path.join(vault, "conc")) if f.startswith("src")]
        R.check("losing moves left their sources in place", len(survivors) == 15, str(len(survivors)))

        # concurrent trash of same-named files from different folders
        for i in range(48):
            c.j("POST", f"/api/notes/ctrash/{i}/note.md", {"content": f"version {i}"})

        def tr(i):
            cc = Client(f"http://{c.host}:{c.port}")
            r = cc.j("POST", "/api/trash", {"path": f"ctrash/{i}/note.md"})
            return i, r.status, (r.json().get("path") if r.status == 200 else None)
        with cf.ThreadPoolExecutor(48) as ex:
            res = list(ex.map(tr, range(48)))
        paths = [p for _, s, p in res if s == 200]
        R.check("concurrent trash: all 48 ok", len(paths) == 48, str({s for _, s, _ in res}))
        R.check("concurrent trash: 48 distinct destinations", len(set(paths)) == 48)
        R.check("concurrent trash: every version intact", all(open(os.path.join(vault, p)).read() == f"version {i}" for i, s, p in res if p))

        # create vs move onto the same destination: whoever answers "success" must not be overwritten
        silent = 0
        for r in range(150):
            c.j("POST", f"/api/notes/cvm/src{r}.md", {"content": "moved"})
            barrier = threading.Barrier(2)

            def do_create():
                cc = Client(f"http://{c.host}:{c.port}")
                barrier.wait()
                return cc.j("POST", f"/api/notes/cvm/dest{r}.md", {"content": "created"}).status

            def do_move():
                cc = Client(f"http://{c.host}:{c.port}")
                barrier.wait()
                return cc.j("POST", "/api/move", {"from": f"cvm/src{r}.md", "to": f"cvm/dest{r}.md"}).status
            with cf.ThreadPoolExecutor(2) as ex:
                fc, fm = ex.submit(do_create), ex.submit(do_move)
                sc, sm = fc.result(), fm.result()
            if sc == 201 and sm == 200:
                silent += 1  # both told "success": one of the two files was silently destroyed
        R.check("create vs move onto one destination: never both succeed (no silent overwrite), 150 rounds", silent == 0, f"{silent} rounds lost a file")

        # concurrent saves while moving the folder
        c.j("POST", "/api/notes/cmv/f/x.md", {"content": "x"})
        stop = threading.Event()
        errors = []

        def saver():
            cc = Client(f"http://{c.host}:{c.port}")
            while not stop.is_set():
                cur = cc.get("/api/notes/cmv/f/x.md")
                if cur.status != 200:
                    continue
                r = cc.j("PUT", "/api/notes/cmv/f/x.md", {"content": "y", "mtime": cur.json()["mtime"]})
                if r.status not in (200, 404, 409):
                    errors.append(r.status)
        ths = [threading.Thread(target=saver) for _ in range(4)]
        [t.start() for t in ths]
        time.sleep(0.3)
        mvr = c.j("POST", "/api/move", {"from": "cmv/f", "to": "cmv/g"})
        time.sleep(0.3)
        stop.set()
        [t.join() for t in ths]
        R.check("move during saves: 200", mvr.status == 200)
        R.check("move during saves: no 5xx", not errors, str(errors))
        R.check("move during saves: old folder not resurrected", not os.path.exists(os.path.join(vault, "cmv/f")))

    section("concurrency", concurrency)


# ----------------------------------------------------------------------------------------
# First-run setup (server started WITHOUT a vault)
# ----------------------------------------------------------------------------------------

def run_setup(c: Client, R: Recorder, home: str, config_dir: str):
    R.rec("setup status before", c.get("/api/setup"))
    for p in ["/api/tree", "/api/notes/a.md", "/api/search?q=x", "/api/backlinks?note=a.md", "/api/files/a.md", "/api/relink?from=a&to=b"]:
        R.rec(f"vault endpoint before setup {p}", c.get(p))
    R.rec("POST vault endpoint before setup", c.j("POST", "/api/dirs/x", {}))
    R.rec("events work without a vault (headers)", Resp(200, {}, b""), body=False)
    R.rec("setup tree-like unknown api", c.get("/api/nope"))
    R.rec("setup GET /", c.get("/"), body=False)
    R.rec("setup invalid json", c.req("POST", "/api/setup", body="x", headers=JSON))
    R.rec("setup no content-type", c.req("POST", "/api/setup", body="{}"))
    R.rec("setup empty object", c.j("POST", "/api/setup", {}))
    R.rec("setup unknown mode", c.j("POST", "/api/setup", {"mode": "weird", "path": "~/x"}))
    for path in ["/", "~", "relative/path", "", "/home", "~/", "..", home, os.path.dirname(home)]:
        label = "<HOME>" if path == home else "<HOME-PARENT>" if path == os.path.dirname(home) else path
        R.rec(f"setup existing {label!r}", c.j("POST", "/api/setup", {"mode": "existing", "path": path}))
    R.rec("setup existing missing folder", c.j("POST", "/api/setup", {"mode": "existing", "path": "~/missing"}))
    os.makedirs(os.path.join(home, "occupied"), exist_ok=True)
    open(os.path.join(home, "occupied", "n.md"), "w").write("x")
    R.rec("setup new over non-empty", c.j("POST", "/api/setup", {"mode": "new", "path": "~/occupied"}))
    open(os.path.join(home, "afile"), "w").write("x")
    R.rec("setup existing is a file", c.j("POST", "/api/setup", {"mode": "existing", "path": "~/afile"}))
    r = R.rec("setup new ok", c.j("POST", "/api/setup", {"mode": "new", "path": "~/Notes"}))
    R.check("welcome note created", os.path.exists(os.path.join(home, "Notes/Welcome.md")))
    cfg = os.path.join(config_dir, "quirenote", "config.toml")
    R.check("config.toml written", os.path.exists(cfg) and open(cfg).read() == f'# Quirenote configuration\nvault = "{os.path.join(home, "Notes")}"\n', open(cfg).read() if os.path.exists(cfg) else "missing")
    R.rec("setup status after", c.get("/api/setup"))
    R.rec("tree after setup", c.get("/api/tree"))
    R.rec("setup again -> 403", c.j("POST", "/api/setup", {"mode": "existing", "path": "~"}))
    R.rec("setup again invalid json -> 403", c.req("POST", "/api/setup", body="x", headers=JSON))
    R.rec("save after setup", c.j("PUT", "/api/notes/first.md", {"content": "works"}))
    # events work after setup (watcher started)
    s = socket.create_connection((c.host, c.port), timeout=5)
    s.sendall(b"GET /api/events HTTP/1.1\r\nHost: x\r\n\r\n")
    s.settimeout(0.3)
    time.sleep(0.4)
    try:
        s.recv(65536)
    except socket.timeout:
        pass
    open(os.path.join(home, "Notes/ext.md"), "w").write("x")
    buf = b""
    end = time.time() + 3
    while time.time() < end and b"ext.md" not in buf:
        try:
            buf += s.recv(65536)
        except socket.timeout:
            pass
    R.check("watcher started after first-run setup", b'"path":"ext.md"' in buf, buf.decode(errors="replace")[-200:])
    s.close()


# ----------------------------------------------------------------------------------------
# Diff
# ----------------------------------------------------------------------------------------

# Intentional / accepted differences, by case name (exact) or regex (prefix "re:"). Each needs a reason.
KNOWN_DIVERGENCES = {}


def load_known():
    p = os.path.join(os.path.dirname(os.path.abspath(__file__)), "known_divergences.json")
    if os.path.exists(p):
        return json.load(open(p))
    return {}


def diff(a_path, b_path):
    a, b = json.load(open(a_path)), json.load(open(b_path))
    known = load_known()
    names = list(a["order"]) + [n for n in b["order"] if n not in a["cases"]]
    bad = documented = same = 0
    for n in names:
        ca, cb = a["cases"].get(n), b["cases"].get(n)
        if ca == cb:
            same += 1
            continue
        reason = None
        for k, why in known.items():
            if k == n or (k.startswith("re:") and re.search(k[3:], n)):
                reason = why
                break
        if reason:
            documented += 1
            print(f"DOCUMENTED  {n}  ({reason})")
            continue
        bad += 1
        print(f"DIVERGENCE  {n}")
        if ca is None or cb is None:
            print(f"    only in {'A' if cb is None else 'B'}")
            continue
        for key in ("status", "headers", "body", "extra"):
            if ca.get(key) != cb.get(key):
                if key == "headers":
                    for h in sorted(set(ca[key]) | set(cb[key])):
                        if ca[key].get(h) != cb[key].get(h):
                            print(f"    header {h}: A={ca[key].get(h)!r} B={cb[key].get(h)!r}")
                else:
                    sa, sb = json.dumps(ca.get(key), ensure_ascii=False), json.dumps(cb.get(key), ensure_ascii=False)
                    print(f"    {key}: A={sa[:300]}\n    {' ' * len(key)}  B={sb[:300]}")
    ck_a = {x["name"]: x for x in a["checks"]}
    ck_b = {x["name"]: x for x in b["checks"]}
    fail_a = [n for n, x in ck_a.items() if not x["ok"]]
    fail_b = [n for n, x in ck_b.items() if not x["ok"]]
    print(f"\ncases: {len(names)} compared, {same} identical, {documented} documented, {bad} UNDOCUMENTED divergences")
    print(f"checks: A {len(ck_a) - len(fail_a)}/{len(ck_a)} ok, B {len(ck_b) - len(fail_b)}/{len(ck_b)} ok")
    for n in fail_a:
        print(f"  A fails: {n}  {ck_a[n]['detail']}")
    for n in fail_b:
        print(f"  B fails: {n}  {ck_b[n]['detail']}")
    return 0 if bad == 0 else 1


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("seed")
    s.add_argument("vault")
    r = sub.add_parser("run")
    r.add_argument("--url", required=True)
    r.add_argument("--vault", required=True)
    r.add_argument("--out", required=True)
    r.add_argument("--only", default="")
    st = sub.add_parser("setup")
    st.add_argument("--url", required=True)
    st.add_argument("--home", required=True)
    st.add_argument("--config-dir", required=True)
    st.add_argument("--out", required=True)
    d = sub.add_parser("diff")
    d.add_argument("a")
    d.add_argument("b")
    args = ap.parse_args()
    if args.cmd == "seed":
        seed(args.vault)
        return 0
    if args.cmd == "diff":
        return diff(args.a, args.b)
    c = Client(args.url)
    if args.cmd == "run":
        vault = os.path.realpath(args.vault)
        R = Recorder(vault)
        run_cases(c, R, vault, only=set(filter(None, args.only.split(","))))
    else:
        R = Recorder(os.path.realpath(args.home))
        run_setup(c, R, os.path.realpath(args.home), os.path.realpath(args.config_dir))
    json.dump({"cases": R.cases, "order": R.order, "checks": R.checks}, open(args.out, "w"), indent=1, ensure_ascii=False)
    fails = [x for x in R.checks if not x["ok"]]
    print(f"\n{len(R.cases)} cases recorded, {len(R.checks)} checks, {len(fails)} failed")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
