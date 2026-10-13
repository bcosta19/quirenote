#!/usr/bin/env python3
"""Deterministic synthetic vault generator: the SAME bytes for every server.

    genvault.py --out /tmp/qbench/data/large/vault --index /tmp/qbench/data/large --notes 10000 --pdfs 40 --images 80
    genvault.py --out /tmp/qbench/data/small/vault --index /tmp/qbench/data/small --notes 200 --pdfs 4 --images 8

Notes live in nested folders (area/topic/subtopic), mix Portuguese and English words with accents,
link to each other with [[Name]], [[folder/Name]] and [[Name|alias]] (a zipf-like popularity, so
backlink counts vary a lot), and a few names repeat in different folders (ambiguous links).
Also writes, next to the vault: notes.txt (relative paths), queries.txt (search terms: common,
mid-frequency and rare), linknotes.txt (notes to ask backlinks for) and manifest.json (with a
content hash of the whole vault). Standard library only.
"""
import argparse
import hashlib
import json
import math
import os
import random
import struct
import zlib

WORDS_PT = ("função compilador análise léxica sintática semântica árvore gramática regra produção token símbolo tabela "
            "memória processo thread arquivo diretório rede protocolo pacote conexão servidor cliente requisição resposta "
            "banco dados índice consulta transação isolamento bloqueio concorrência versão conflito histórico revisão "
            "estudo resumo capítulo exercício questão prova leitura anotação ideia projeto tarefa reunião decisão risco "
            "otimização desempenho latência vazão memória cache fila escalonador sistema operacional núcleo kernel").split()
WORDS_EN = ("parser lexer grammar compiler runtime allocator garbage collector heap stack thread mutex channel buffer socket "
            "stream request response header payload cookie session token cache index query planner storage engine "
            "journal replica consensus leader follower snapshot checkpoint recovery benchmark profile flamegraph "
            "refactor design pattern interface module package release changelog review draft summary outline").split()
AREAS = ["Estudos", "Projetos", "Diário", "Livros", "Referências", "Trabalho", "Ideias", "Arquivo", "Redes", "Sistemas",
         "Linguagens", "Bancos", "Segurança", "Algoritmos", "Matemática", "Física", "Leituras", "Cursos", "Pessoal", "Saúde",
         "Finanças", "Viagens", "Receitas", "Música", "Cinema"]


def png(rng, size):
    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    w = h = max(1, int(math.sqrt(size / 3)))
    raw = b"".join(b"\x00" + bytes(rng.getrandbits(8) for _ in range(w * 3)) for _ in range(h))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 1)) + chunk(b"IEND", b""))


def pdf(rng, size):
    body = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n"
    body += b"% " + bytes(rng.getrandbits(8) for _ in range(max(0, size - len(body) - 20))).replace(b"\n", b" ") + b"\n"
    return body + b"%%EOF\n"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--index", required=True)
    ap.add_argument("--notes", type=int, required=True)
    ap.add_argument("--pdfs", type=int, default=0)
    ap.add_argument("--images", type=int, default=0)
    ap.add_argument("--seed", type=int, default=20261013)
    a = ap.parse_args()
    if not a.out.startswith("/tmp/qbench/"):
        raise SystemExit("refusing to write outside /tmp/qbench")
    rng = random.Random(a.seed)
    n = a.notes

    # nested layout: about 10 notes per leaf folder
    leaves = max(1, n // 10)
    top = max(1, round(leaves ** (1 / 3)))
    mid = max(1, round((leaves / top) ** 0.5))
    sub = max(1, math.ceil(leaves / (top * mid)))
    paths, names = [], []
    for i in range(n):
        leaf = i % (top * mid * sub)
        t, m, s = leaf % top, (leaf // top) % mid, leaf // (top * mid)
        area = AREAS[t % len(AREAS)] + ("" if t < len(AREAS) else f" {t // len(AREAS) + 1}")
        base = rng.choice(WORDS_PT + WORDS_EN).capitalize() + " " + rng.choice(WORDS_PT + WORDS_EN)
        name = f"{base} {i}" if rng.random() > 0.01 else base  # ~1% of the names repeat across folders
        folder = f"{area}/Tópico {m}/Sub {s}"
        paths.append(f"{folder}/{name}.md")
        names.append(name)
    # popularity (zipf-like) for link targets
    weights = [1 / (r + 1) ** 0.9 for r in range(n)]
    order = list(range(n))
    rng.shuffle(order)
    rare_words = [f"zq{rng.randrange(10**6):06d}" for _ in range(12)]

    def paragraph(k):
        ws = [rng.choice(WORDS_PT if rng.random() < .6 else WORDS_EN) for _ in range(k)]
        return " ".join(ws).capitalize() + "."

    files = {}
    for i in range(n):
        lines = [f"# {names[i]}", "", f"tags: #{rng.choice(WORDS_EN)} #{rng.choice(WORDS_PT)}", ""]
        for _ in range(rng.randint(3, 9)):
            p = paragraph(rng.randint(12, 40))
            if rng.random() < 0.55:
                tgt = order[rng.choices(range(n), weights)[0]]
                form = rng.random()
                link = f"[[{names[tgt]}]]" if form < .6 else f"[[{paths[tgt][:-3]}]]" if form < .8 else f"[[{names[tgt]}|{rng.choice(WORDS_PT)}]]"
                p += f" Ver {link}."
            lines.append(p)
            lines.append("")
        if rng.random() < 0.15:
            lines += ["```python", "def f(x):", f"    return x * {i}", "```", ""]
        if rng.random() < 0.004 * len(rare_words):
            lines.append(f"Marcador raro: {rng.choice(rare_words)}")
        files[paths[i]] = "\n".join(lines) + "\n"
    for p in range(a.pdfs):
        files[f"Livros/PDF/Livro {p}.pdf"] = pdf(rng, rng.randint(60_000, 240_000))
        # a note that links the page of the pdf
        k = rng.randrange(n)
        files[paths[k]] += f"\nLeitura: [[Livro {p}.pdf#page={rng.randint(1, 300)}]]\n"
    for p in range(a.images):
        files[f"Anexos/Imagem {p}.png"] = png(rng, rng.randint(2_000, 40_000))
        k = rng.randrange(n)
        files[paths[k]] += f"\n![[Imagem {p}.png]]\n"

    h = hashlib.sha256()
    total = 0
    for rel in sorted(files):
        data = files[rel].encode() if isinstance(files[rel], str) else files[rel]
        full = os.path.join(a.out, rel)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, "wb") as f:
            f.write(data)
        h.update(rel.encode() + b"\0" + hashlib.sha256(data).digest())
        total += len(data)
    os.makedirs(os.path.join(a.out, ".obsidian"), exist_ok=True)
    open(os.path.join(a.out, ".obsidian", "app.json"), "w").write("{}")
    os.makedirs(a.index, exist_ok=True)
    note_list = sorted(paths)
    open(os.path.join(a.index, "notes.txt"), "w").write("\n".join(note_list) + "\n")
    # backlinks: the most linked notes and some random ones
    open(os.path.join(a.index, "linknotes.txt"), "w").write("\n".join(paths[j] for j in order[:50] + rng.sample(range(n), min(50, n))) + "\n")
    common = ["função", "memória", "thread", "request", "cache", "projeto", "sistema", "query", "design", "versão"]
    mid_words = [rng.choice(WORDS_PT + WORDS_EN) + " " + rng.choice(WORDS_PT + WORDS_EN) for _ in range(10)]
    queries = common + mid_words + rare_words[:10]
    open(os.path.join(a.index, "queries.txt"), "w").write("\n".join(queries) + "\n")
    json.dump({"notes": n, "pdfs": a.pdfs, "images": a.images, "files": len(files), "bytes": total, "sha256": h.hexdigest(),
               "layout": {"top": top, "mid": mid, "sub": sub}, "seed": a.seed}, open(os.path.join(a.index, "manifest.json"), "w"), indent=1)
    print(f"{len(files)} files, {total/1e6:.1f} MB, sha256 {h.hexdigest()[:16]} -> {a.out}")


if __name__ == "__main__":
    main()
