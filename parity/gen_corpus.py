#!/usr/bin/env python3
"""Generates the parity corpus: a systematic matrix over every diagnostic
check both engines implement (operators × nullability × key placement),
plus duplicates, missing endpoints, records constraints, groups, and the
real reported cases. Deterministic — re-running rewrites the same files."""
import os

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "corpus")
os.makedirs(OUT, exist_ok=True)

cases = {}

def add(name, src):
    assert name not in cases, f"dup case {name}"
    cases[name] = src.strip() + "\n"

OPS = [">", "<", "-", "<>", ">?", "<?", "<>?"]

# --- operators × nullability on both sides ---------------------------------
for op in OPS:
    for fk_attr, fk_label in [("", "unspec"), ("[not null]", "nn"), ("[null]", "null")]:
        for par_attr, par_label in [("", "unspec"), ("[not null]", "nn")]:
            add(
                f"op-{op.replace('<', 'lt').replace('>', 'gt').replace('-', 'dash').replace('?', 'q')}-{fk_label}-{par_label}",
                f"Table parent {{ id integer {par_attr}\n}}\n"
                f"Table child {{ parent_id integer {fk_attr}\n}}\n"
                f"Ref: parent.id {op} child.parent_id\n",
            )

# --- key placement: where the referenced column's uniqueness lives ---------
KEYS = {
    "none": "",
    "col-pk": "[pk]",
    "col-unique": "[unique]",
    "idx-pk": "indexes { (id) [pk] }",
    "idx-unique": "indexes { (id) [unique] }",
}
for key, body in KEYS.items():
    # single-line parent: a split-line setting reads as a broken column
    parent = f'Table parent {{ id integer {body} }}' if body else 'Table parent { id integer }'
    add(
        f"key-{key}-lt",
        f'{parent}\n'
        f'Table child {{ parent_id integer [not null]\n}}\n'
        f'Ref: parent.id < child.parent_id\n',
    )
    # the inline form: the ref rides INSIDE the settings brackets
    # (dbdiagram syntax: [not null, ref: > parent.id])
    add(
        f"key-{key}-inline-gt",
        f'{parent}\n'
        f'Table child {{ parent_id integer [not null, ref: > parent.id]\n}}\n',
    )

# composite keys: satisfied vs missing
add(
    "key-composite-ok",
    "Table parent { id integer [pk]\n  code integer [pk]\n}\n"
    "Table child { parent_id integer [not null]\n  code integer [not null]\n}\n"
    "Ref: parent.(id, code) > child.(parent_id, code)\n",
)
add(
    "key-composite-missing",
    "Table parent { id integer [pk]\n  code integer\n}\n"
    "Table child { parent_id integer [not null]\n  code integer [not null]\n}\n"
    "Ref: parent.(id, code) > child.(parent_id, code)\n",
)

# --- duplicates -------------------------------------------------------------
add("dup-table", "Table a { id integer }\nTable a { x integer }\n")
add("dup-enum", "Enum e { x }\nEnum e { y }\n")
add("dup-table-and-enum", "Table t { id integer }\nEnum t { x }\n")

# --- missing endpoints ------------------------------------------------------
add("ref-missing-table", "Table a { id integer }\nRef: a.id > missing.id\n")
add("ref-missing-column", "Table a { id integer }\nTable b { x integer }\nRef: a.id > b.nope\n")
add("ref-missing-composite-half", "Table a { id integer }\nTable b { x integer }\nRef: a.(id, nope) > b.(x, y)\n")

# --- records constraints ----------------------------------------------------
add("records-fk-violation",
    "Table users { id integer [pk]\n}\n"
    "Table posts { id integer [pk]\n  author_id integer [ref: > users.id]\n"
    "  records (id, author_id) {\n    1, 42\n  }\n}\n")
add("records-fk-clean",
    "Table users { id integer [pk]\n  records (id) {\n    1\n  }\n}\n"
    "Table posts { id integer [pk]\n  author_id integer [ref: > users.id]\n"
    "  records (id, author_id) {\n    1, 1\n  }\n}\n")
add("records-null-in-required",
    "Table users { id integer [pk]\n}\n"
    "Table posts { id integer [pk]\n  author_id integer [not null, ref: > users.id]\n"
    "  records (id, author_id) {\n    1, ''\n  }\n}\n")

# --- groups -----------------------------------------------------------------
add("group-ok", 'Table a { id integer }\nTable b { id integer }\n'
    'TableGroup g { a, b }\n')
add("group-missing-member", 'Table a { id integer }\nTableGroup g { a, nope }\n')

# --- enums ------------------------------------------------------------------
add("enum-duplicate-member", "Enum e { x\n  x\n}\n")
add("enum-ref-member", "Table t { s e [ref: - e.x]\n}\nEnum e { x }\n")

# --- clean baselines --------------------------------------------------------
add("clean-minimal", "Table a { id integer [pk]\n  name varchar\n}\n")

# --- real reported cases ----------------------------------------------------
add("reported-index-pk",
    'Table "test1" {\n  "id" INT [not null]\n\n  indexes {\n    ("id") [pk]\n  }\n}\n'
    '\nTable "test2" {\n  "id" INT [not null]\n}\n'
    '\nRef: "test1"."id" < "test2"."id"\n')

for name, src in sorted(cases.items()):
    with open(os.path.join(OUT, f"{name}.dbml"), "w") as f:
        f.write(src)
print(f"wrote {len(cases)} corpus cases to {OUT}")
