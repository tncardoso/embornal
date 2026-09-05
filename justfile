site:
    uv sync --locked --no-config --only-group dev --no-install-project --python 3.13
    hugo --source site --cleanDestinationDir --gc --minify --panicOnWarning
    uv run --no-sync zensical build --clean --strict
    test -f site/public/index.html
    test -f site/public/docs/index.html

serve:
    #!/usr/bin/env bash
    set -euo pipefail
    uv sync --locked --no-config --only-group dev --no-install-project --python 3.13
    trap 'kill 0' EXIT
    hugo serve --source site &
    uv run --no-sync zensical serve &
    wait
