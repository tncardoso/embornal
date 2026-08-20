# Website

The published website has two parts:

- Hugo builds the landing page at `/`.
- Zensical builds the documentation at `/docs/`.

Both generators write to `site/public/`. Hugo must run before Zensical. The
Hugo build cleans `site/public/`. The Zensical build then writes only to
`site/public/docs/`.

## Install the build tools

Install these tools:

- Just 1.57.0
- Hugo Extended 0.164.0
- uv 0.12.0

The uv lock file selects Python 3.13 and Zensical 0.0.51.

The synchronization command uses `--no-config`. This option tells uv to
ignore the user configuration files. A user configuration that sets
`exclude-newer` writes an `[options]` section into the lock file. The lock
file then fails the `--locked` check on a machine that does not have the same
configuration, such as the continuous integration runner. The `--no-config`
option makes each machine resolve the same lock file.

## Build the complete website

Run this command from the repository root:

```console
$ just site
```

The command synchronizes the locked documentation dependencies. It then
builds Hugo before it builds Zensical. The command stops if a generator gives
a warning or if an expected entry page does not exist.

The generated files are:

```text
site/public/index.html
site/public/docs/index.html
```

The `site/public/` directory is generated output. Do not commit its contents.

## Preview the complete website

Build the website. Then serve the generated directory:

```console
$ python -m http.server --directory site/public 8000
```

Open `http://localhost:8000/` for the landing page. Open
`http://localhost:8000/docs/` for the documentation.

Do not open the generated HTML files directly. The website uses paths that
start at the host root.

## Change the landing page

The `site/` directory contains the Hugo project. Edit
`site/themes/embornal/layouts/index.html` to change the page content. Edit
`site/themes/embornal/assets/css/main.css` to change its appearance.
The landing-page header shows the Embornal name and menu items in white.

The Hugo configuration mounts `docs/assets/` at `/assets/`. Thus, the landing
page and the documentation use the same banner source file.

Zensical reads the Markdown files in `docs/`. The header override is in
`overrides/partials/header.html`. The header has a `Home` link. This link
opens the Hugo landing page at the host root (`/`).

## Deploy the website

The Documentation workflow builds pull requests that change website inputs.
It does not deploy pull requests. A push to `main` builds and deploys
`site/public/` as one GitHub Pages artifact.

The production URL is `https://embornal.com/`. The documentation URL is
`https://embornal.com/docs/`.
