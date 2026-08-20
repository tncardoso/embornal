site:
    uv sync --locked --only-group dev --no-install-project --python 3.13
    hugo --source site --cleanDestinationDir --gc --minify --panicOnWarning
    uv run --no-sync zensical build --clean --strict
    test -f site/public/index.html
    test -f site/public/docs/index.html
