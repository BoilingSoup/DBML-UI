#!/usr/bin/env python3
"""The embedded-engine fidelity gate: the bundled dbml-parse under Boa
must produce the same full-span diagnostics as the node reference over
the same cases. Usage: boa_gate.py <boa-out.json> <node-out.json>."""

import json
import sys


def load(fp, keyf):
    raw = json.load(open(fp))
    if isinstance(raw, str):  # the double-serialized form
        raw = json.loads(raw)
    out = {}
    for it in raw:
        key = keyf(it).split("/")[-1]
        out[key] = sorted(
            (d["type"], d["text"], d.get("row"), d.get("column"), d.get("endRow"), d.get("endColumn"))
            for d in it["diagnostics"]
        )
    return out


def main():
    boa = load(sys.argv[1], lambda it: (it.get("name") or it.get("path", "")))
    ref = load(sys.argv[2], lambda it: (it.get("name") or it.get("path", "")))
    bad = [k for k in sorted(set(boa) | set(ref)) if boa.get(k) != ref.get(k)]
    print(f"embedded-vs-reference: {len(ref) - len(bad)}/{len(ref)} identical")
    assert not bad, bad[:8]


if __name__ == "__main__":
    main()
