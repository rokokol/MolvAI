# MolvAI website

![MolvAI](assets/logo.svg)

The static marketing and documentation site for MolvAI. Plain HTML on the DDLC web kit, one site stylesheet, two scripts and SVG icons. No build step, no package manager, no CDN, no cookies and no analytics. The product pages are written in English and Russian

## Preview

```sh
rm -rf /tmp/molvai-site
mkdir -p /tmp/molvai-site/docs
cp -R site/. /tmp/molvai-site/
cp -R docs/img /tmp/molvai-site/docs/img
python3 -m http.server -d /tmp/molvai-site
```

The copy is the assembly that `.github/workflows/pages.yml` performs. The home page shows the screenshots in `docs/img/`, which is not in `site/`, so a preview of `site/` alone leaves four images broken: serve such a copy, never `site/` itself. `.gitignore` rejects `site/docs/img`, so a link that points at the screenshots cannot be committed by accident. `file://` still opens every page with its English text, but the module script that draws the clouds does not run from a file URL

## How it deploys

`.github/workflows/pages.yml` publishes `site/` to GitHub Pages on every push to `main`, and by hand from the Actions tab. The workflow assembles an artifact: it copies `site/` into `_site/`, then copies `docs/img/` from the repository into `_site/docs/img/`. It uploads that directory with `actions/upload-pages-artifact` and deploys it with `actions/deploy-pages`

The concurrency group is `pages` with `cancel-in-progress: false`, so a newer run waits for the running one and publishes last. An older deploy cannot overwrite a newer one, and a deploy is never killed halfway

Nothing in the artifact is generated from the pages. What is in `site/` is what is served, plus the copied screenshots

## The look

The visual system is the owner's DDLC web kit from [ddlc-themes](https://github.com/rokokol/ddlc-themes), vendored unchanged into `assets/vendor/ddlc/`. `ddlc-tokens.css` and `ddlc-ui.css` lay a page out of `.bar`, `.work`, `.card`, `.code`, `.tabs`, `.segmented`, `dialog` and `.cloud`; `ddlc-report.css` brings the prose sheet, which is the taped quote, the kitty code window and the "Just Monika." pop-up. A page links all three and then `assets/site.css`

`assets/site.css` is the site's own layer and holds no colour of its own: every colour is a `--ddlc-*` role. It lifts the report sheet's 46rem column, because the kit lays the page out, and it adds only what the kit has no class for: the hero, the stickers, the spectrogram pop-up, the documentation layout and the footer

- The cloud of light at the top is `startCloud` from `ddlc-cloud.js` on `<canvas class="cloud">`, plus a sky canvas in the hero that `kit.js` draws with the kit's `ditherDisc` and `bayerThresholds`. Both read their role through the kit's `color()` probe, so a theme change recolours them
- The spectrogram is a native `<dialog>` with a canvas: a lattice of lit cells in the accent roles, animated with `requestAnimationFrame`, stopped on close, on Escape and on a hidden tab, and drawn as one still frame under `prefers-reduced-motion`
- A screenshot is a `figure.ddlc-snap` with a `<span class="tape">`. The vendored CSS does not carry `.ddlc-snap` yet, so `site.css` holds that rule in a block marked as a temporary bridge and nothing else: when the vendor copy adds the class, delete the block
- A number or a source is a `.sticker` or a `.num-sticker`; a principle is a `<blockquote>`, which the report sheet tapes on by itself
- A code block is a bare `<pre data-lang="...">`, which the report sheet draws as a terminal window, and a one-line command is `.cmd` with its prompt and copy button

## Languages

Every translatable element keeps its English text in the markup and its Russian in a `data-ru` attribute beside it. A translated attribute is `data-ru-aria-label`, `data-ru-alt`, `data-ru-title` or `data-ru-content`, and `<title>` and `<meta name="description">` carry the pair the same way. Code, commands, flags, paths, numbers and product names are the same in both

`assets/site.js` collects the English, applies the chosen language on load, and swaps back and forth on the header switch. It writes the choice to `molvai-lang`, sets `<html lang>`, puts `?lang=` back into the address with `history.replaceState`, and dispatches a `molvai:lang` event so `kit.js` can relabel the theme button

The order of preference is `?lang`, then the stored choice, then the browser's language list, then English. With JavaScript off the markup is English, which is why English is the fallback rather than an option. The head script still applies a pinned `?theme=light|dark` before the first paint, and still sets `<html lang="ru">` for `?lang=ru`

Every page is complete in both languages: the product pages, the documentation hub and its nine pages, and the 404. The 404 is the one file that cannot link the shared stylesheet, because GitHub Pages serves it at whatever depth the reader asked for: it carries the language protocol, a cut-down palette and the icon inline, and a comment in it says so

## Where the facts live

The repository is canonical. Every docs page carries a visible "canonical source" link to the file that owns its subject:

- `README.md` for the overview, the settings table and the project's own features
- `CHANGELOG.md` for what changed in each release
- `docs/ARCHITECTURE.md` for the pipeline and the processes
- `docs/install.md` for installation as a step-by-step prompt
- `docs/journal-schema.md` for every journal field
- `docs/hotkeys-wayland.md` for the compositor binds
- `models.toml` for the model catalog, which the CLI, the GUI and the Nix module also read
- `CHANGELOG.md` for released changes

When a page and the repository disagree, fix the page. Do not restate a value in two places on the site: the model catalog lives on `models.html`, and `docs/models.html` links to it instead of copying the table

## Editing rules

- Relative paths only. A link starts with `./` or `../`, never with `/`, because the same files are served from `https://rokokol.github.io/MolvAI/` and later from the root of the `molvai.rokokol.art` subdomain
- No external runtime dependency. The two fonts and every stylesheet and script are vendored in this repository: no Google Fonts, no CDN, no tracker, no analytics, no cookie
- The only stored values are the theme under `molvai-theme` and the language under `molvai-lang`, both in `localStorage`. A `?theme=` or `?lang=` parameter overrides them for one address and pins the choice
- `assets/site.js` carries the language switch, the copy buttons and the install tabs. `assets/kit.js` is the module that wires the vendored kit: the clouds, the theme button and the spectrogram. With both blocked every page shows English, the tabs stack as panels, the copy buttons and the spectrogram button disappear
- Duplicate the header and the footer from an existing page when you add one. The paths differ by one `../` inside `docs/`
- A translatable element is written as English markup plus a `data-ru` attribute with the Russian HTML, escaped once for the attribute. An element that is not translated has no `data-ru` and stays as it is in both languages
- Do not edit `assets/vendor/ddlc/`: it is upstream's, and `ASSETS.md` holds the font licences it brings
- Set `aria-current="page"` on exactly one link per navigation, and use `aria-current="page"` in the docs sidebar for the current page
- Give every table a `<caption>` and every header cell a `scope`
- Keep one `<h1>` per page and one thought per paragraph. Prose is English, sentences are short, and a paragraph ends without a full stop, as in the rest of the repository

## Files

- `index.html` doubles as the landing page and the install section
- `features.html`, `models.html`, `cli.html`, `faq.html` are the product pages
- `docs/` holds the documentation hub and its pages, with a sidebar on desktop
- `404.html` is deliberately self-contained: GitHub Pages serves it at the requested address, so relative links to shared assets break below the first path segment. Its inline styles keep it readable everywhere
- `assets/site.css` holds the site's layer over the kit, and `assets/kit.js` plus `assets/site.js` hold its behaviour
- `assets/vendor/ddlc/` is the vendored DDLC web kit and its two fonts, with their licences in the same directory and the summary in `ASSETS.md`
- `assets/logo.svg` and `assets/icon-github.svg` are the only images in `site/`. `logo.svg` serves as the favicon too, so the mark exists once. A raster screenshot is not in the site on purpose: `docs/img/` arrives in the artifact through the workflow, so `index.html` uses `./docs/img/panel-idle-2026.png` from the site root without a second copy in the repository

## The subdomain

Do not add a `CNAME` file now. When the owner points `molvai.rokokol.art` at the site, these things change:

1. A DNS record at the domain: `CNAME` for `molvai` pointing at `rokokol.github.io`
2. A file `site/CNAME` whose single line is `molvai.rokokol.art`, which the workflow copies into the artifact with the rest of `site/`

Then set the custom domain in the repository's Pages settings and enable the certificate. No page needs an edit: every path is relative, so the site moves from the project path to the subdomain root unchanged
