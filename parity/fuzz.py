#!/usr/bin/env python3
"""Tier-2 differential fuzzing: generates STRUCTURED (grammar-shaped)
DBML — both engines should largely agree on parse status, so the
diagnostic/schema comparison stays meaningful — writes the cases to
parity/fuzz/, and the standard runner diffs them:

    python3 parity/fuzz.py 200 && node parity/run.mjs

Byte-level mutation fuzzing lives in the Rust tier (tests/fuzz_smoke.rs);
here the generator stays grammar-shaped because broken-input parse-error
messages are a different (and low-value) porting surface. Deterministic
per seed."""
import os
import random
import sys

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fuzz")

NAMES = ["users", "posts", "a", "B", "my_table", '"quoted name"', "ñT", "_x", "t1", "orders"]
COLS = ["id", "name", "x", '"odd col"', "user_id", "code"]
TYPES = ["integer", "varchar(255)", "decimal(10, 2)", '"character varying"', "e_status",
         "text", "timestamp", "bool", "int[]"]
OPS = [">", "<", "-", "<>", ">?", "<?", "?>", "?<", "-?", "?-", "<>?", "?<>" ]
SETTINGS = ["pk", "unique", "not null", "null", "increment", "default: 'x'", "default: 1",
            "default: `now()`", "note: 'n'", "check: `x > 0`", "color: #1E69FD"]


def gen_case(rng):
    lines = []
    n_tables = rng.randint(1, 4)
    tables = []
    for t in range(n_tables):
        name = rng.choice(NAMES)
        tables.append(name)
        lines.append(f"Table {name} {{")
        cols = []
        for _ in range(rng.randint(1, 4)):
            col = rng.choice(COLS)
            cols.append(col)
            st = ""
            if rng.random() < 0.75:
                picked = rng.sample(SETTINGS, rng.randint(1, 2))
                if rng.random() < 0.3:
                    target = rng.choice(tables + ["nope"])
                    picked.append(f"ref: {rng.choice(OPS)} {target.strip(chr(34))}.{rng.choice(cols).strip(chr(34))}")
                st = f" [{', '.join(picked)}]"
            elif rng.random() < 0.2:
                st = f" {rng.choice(['unique', 'pk', 'foo'])}"
            lines.append(f"  {col} {rng.choice(TYPES)}{st}")
        if rng.random() < 0.4:
            lines.append("  indexes {")
            for _ in range(rng.randint(1, 2)):
                cc = rng.choice(cols).strip('"')
                st = rng.choice(["", " [unique]", " [pk]", " [unique, name: 'ix']"])
                lines.append(f"    ({cc}){st}" if rng.random() < 0.5 else f"    {cc}{st}")
            lines.append("  }")
        if rng.random() < 0.25:
            lines.append("  Note: 'table note'")
        if rng.random() < 0.2:
            c0 = cols[0].strip('"')
            lines.append(f"  records ({c0}) {{")
            for _ in range(rng.randint(1, 3)):
                v = rng.choice(["1", "'a'", "null", "2.5", "true", "`x`", "''"])
                lines.append(f"    {v}")
            lines.append("  }")
        lines.append("}")
    for r in range(rng.randint(0, 2)):
        a = rng.choice(tables).strip('"')
        b = rng.choice(tables + ['"missing"']).strip('"')
        name = f" r{r}" if rng.random() < 0.3 else ""
        lines.append(f"Ref{name}: {a}.{rng.choice(COLS).strip(chr(34))} "
                     f"{rng.choice(OPS)} {b}.{rng.choice(COLS).strip(chr(34))}")
    if rng.random() < 0.4:
        lines.append("Enum e_status {\n  open\n  closed [note: 'done']\n}")
    if rng.random() < 0.3:
        members = ", ".join(t.strip('"') for t in tables)
        lines.append(f"TableGroup g {{ {members} }}")
    if rng.random() < 0.2:
        lines.append("Note sticky {\n  '''\n  text\n  '''\n}")
    return "\n".join(lines) + "\n"


def main():
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 200
    seed = int(sys.argv[2]) if len(sys.argv) > 2 else 20241108
    os.makedirs(OUT, exist_ok=True)
    for f in os.listdir(OUT):
        os.remove(os.path.join(OUT, f))
    rng = random.Random(seed)
    for i in range(n):
        with open(os.path.join(OUT, f"fuzz{i:04}.dbml"), "w") as f:
            f.write(gen_case(rng))
    print(f"wrote {n} fuzz cases to {OUT}")


if __name__ == "__main__":
    main()
