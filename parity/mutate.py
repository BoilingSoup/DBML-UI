#!/usr/bin/env python3
"""Byte-level differential mutation campaign: seeds = the corpus + the
probe battery + the fuzz corpus; mutations = deletions, duplications,
token insertions, line swaps, truncations. Compared at FULL span
fidelity. Deterministic per seed."""
import json, os, random, sys, hashlib

SEEDS = []
for d in ["/home/boilingsoup/Dev/dbml-ui/parity/corpus",
          "/tmp/swallow",
          "/home/boilingsoup/Dev/dbml-ui/parity/fuzz"]:
    for f in sorted(os.listdir(d)):
        if f.endswith(".dbml"):
            SEEDS.append(os.path.join(d, f))

TOKENS = ["}", "{", "]", "[", ")", "(", ",", ":", "'", '"', "`", ";",
          "Table x {", "records (a) {", "Ref: a.b > c.d", "note: 'x",
          "ref: > a.b", " ", "\n", "  ", "ref: ", "default: 1", "pk",
          "TableGroup g {", "Enum e {", "Note n {", "Project p {", "'''"]

def mutate(src, rng):
    b = src.encode("utf-8")
    for _ in range(rng.randint(1, 3)):
        if not b: break
        op = rng.choice(["del", "dup_line", "del_line", "ins_tok", "swap", "trunc", "ins_nl"])
        if op == "del":
            i = rng.randrange(len(b)); n = min(rng.randint(1, 8), len(b) - i)
            b = b[:i] + b[i+n:]
        elif op == "dup_line":
            lines = b.split(b"\n"); i = rng.randrange(len(lines))
            lines.insert(i, lines[i]); b = b"\n".join(lines)
        elif op == "del_line":
            lines = b.split(b"\n"); i = rng.randrange(len(lines))
            del lines[i]; b = b"\n".join(lines)
        elif op == "ins_tok":
            i = rng.randrange(len(b) + 1)
            t = rng.choice(TOKENS).encode()
            b = b[:i] + t + b[i:]
        elif op == "swap":
            lines = b.split(b"\n")
            if len(lines) >= 2:
                i, j = rng.sample(range(len(lines)), 2)
                lines[i], lines[j] = lines[j], lines[i]
            b = b"\n".join(lines)
        elif op == "trunc":
            i = rng.randrange(len(b)); b = b[:i]
        elif op == "ins_nl":
            i = rng.randrange(len(b) + 1)
            b = b[:i] + b"\n" + b[i:]
    return b.decode("utf-8", errors="replace")

def main():
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 500
    seed = int(sys.argv[2]) if len(sys.argv) > 2 else 42
    rng = random.Random(seed)
    out = "/tmp/mut"
    os.makedirs(out, exist_ok=True)
    cases = []
    for i in range(n):
        src = open(rng.choice(SEEDS), encoding="utf-8").read()
        m = mutate(src, rng)
        name = f"m{seed}_{i:04d}"
        path = os.path.join(out, name + ".dbml")
        open(path, "w", encoding="utf-8").write(m)
        cases.append({"name": name, "path": path})
    json.dump(cases, open("/tmp/mut-cases.json", "w"))
    print(len(cases), "cases written")

main()
