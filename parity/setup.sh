#!/bin/sh
# Sets up the parity harness: clones holistics/dbml at the pinned commit
# (parity/upstream-pin.txt) and wires the driver's node_modules into it
# so the upstream sources resolve their runtime deps.
set -e
cd "$(dirname "$0")"
SHA=$(cat upstream-pin.txt)
if [ ! -d upstream/dbml/.git ]; then
  git clone https://github.com/holistics/dbml.git upstream/dbml
fi
cd upstream/dbml
git fetch origin "$SHA" 2>/dev/null || true
git checkout -q "$SHA"
ln -sfn ../../driver/node_modules node_modules
cd ../..
echo "upstream ready at $SHA"
