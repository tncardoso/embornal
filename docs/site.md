# Landing page

The `site/` directory contains the Embornal landing page. Hugo builds the
page with the `embornal` theme in `site/themes/embornal/`.

## Build the page

Install Hugo Extended 0.146.0 or a later version. Then run:

```console
$ cd site
$ hugo
```

Hugo writes the generated page to `site/public/`.
The site uses `/` as its base URL. Thus, the landing page and its assets are
served from the root of the host.

To inspect changes while you edit the theme, run:

```console
$ cd site
$ hugo server
```

Open `http://localhost:1313/`.

## Change the theme

Edit `site/themes/embornal/layouts/index.html` to change the page content.
Edit `site/themes/embornal/assets/css/main.css` to change the appearance.

The site configuration mounts `docs/assets/` at `/assets/`. Thus, the landing
page uses the same `banner.png` file as the project documentation.

The hero explains that Embornal keeps facts in contextual wiki paths. The
GitHub links that are named `GitHub` open the project in a new browser tab.
