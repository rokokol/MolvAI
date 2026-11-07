# MolvAI website

![MolvAI](assets/logo.svg)

The static marketing and documentation site for MolvAI. Plain HTML, one stylesheet, one script, and SVG icons. No build step, no package manager, no CDN, no cookies and no analytics

## Preview

```sh
python3 -m http.server -d site
```

Open the printed address. Every page is a plain file, so `file://` works too, except that the copy buttons need a secure context or a fallback that `file://` does not always give

## How it deploys

`.github/workflows/pages.yml` publishes `site/` to GitHub Pages on every push to `main`, and by hand from the Actions tab. The workflow assembles an artifact: it copies `site/` into `_site/`, then copies `docs/img/` from the repository into `_site/docs/img/`. It uploads that directory with `actions/upload-pages-artifact` and deploys it with `actions/deploy-pages`

The concurrency group is `pages` with `cancel-in-progress: false`, so a newer run waits for the running one and publishes last. An older deploy cannot overwrite a newer one, and a deploy is never killed halfway

Nothing in the artifact is generated from the pages. What is in `site/` is what is served, plus the copied screenshots

## Where the facts live

The repository is canonical. Every docs page carries a visible "canonical source" link to the file that owns its subject:

- `README.md` for the overview, the settings table and the project's own features
- `docs/STATUS.md` for what works and what is cut
- `docs/ARCHITECTURE.md` for the pipeline and the processes
- `docs/install.md` for installation as a step-by-step prompt
- `docs/journal-schema.md` for every journal field
- `docs/hotkeys-wayland.md` for the compositor binds
- `models.toml` for the model catalog, which the CLI, the GUI and the Nix module also read
- `CHANGELOG.md` for released changes

When a page and the repository disagree, fix the page. Do not restate a value in two places on the site: the model catalog lives on `models.html`, and `docs/models.html` links to it instead of copying the table

## Editing rules

- Relative paths only. A link starts with `./` or `../`, never with `/`, because the same files are served from `https://rokokol.github.io/MolvAI/` and later from the root of the `molvai.rokokol.art` subdomain
- No external runtime dependency. System font stacks only, no Google Fonts, no CDN script, no tracker, no analytics, no cookie
- The only stored value is the theme preference, under the `molvai-theme` key in `localStorage`
- All JavaScript is in `assets/site.js`. It adds the mobile menu, the theme toggle, the copy buttons and the install tabs, and every page works with JavaScript off: the menu and the theme button hide themselves, the tabs render as stacked panels and the copy buttons disappear
- Duplicate the header and the footer from an existing page when you add one. The paths differ by one `../` inside `docs/`
- Set `aria-current="page"` on exactly one link per navigation, and use `aria-current="page"` in the docs sidebar for the current page
- Give every table a `<caption>` and every header cell a `scope`
- Keep one `<h1>` per page and one thought per paragraph. Prose is English, sentences are short, and a paragraph ends without a full stop, as in the rest of the repository

## Files

- `index.html` doubles as the landing page and the install section
- `features.html`, `models.html`, `cli.html`, `faq.html` are the product pages
- `docs/` holds the documentation hub and its pages, with a sidebar on desktop
- `404.html` is deliberately self-contained: GitHub Pages serves it at the requested address, so relative links to shared assets break below the first path segment. Its inline styles keep it readable everywhere
- `assets/site.css` holds the palette. Dark is the default, the light theme follows the system setting or the toggle, and the accent comes from the product palette in `crates/molva-gui/ui/src/styles.css`
- `assets/site.js` is progressive enhancement only
- `assets/favicon.svg`, `assets/logo.svg` and `assets/icon-github.svg` are the only images. A raster screenshot is not in the site on purpose: `docs/img/` arrives in the artifact through the workflow, so a page can use `./docs/img/panel-idle.png` from the site root without a second copy in the repository. No page does that yet

## The subdomain

Do not add a `CNAME` file now. When the owner points `molvai.rokokol.art` at the site, these things change:

1. A DNS record at the domain: `CNAME` for `molvai` pointing at `rokokol.github.io`
2. A file `site/CNAME` whose single line is `molvai.rokokol.art`, which the workflow copies into the artifact with the rest of `site/`

Then set the custom domain in the repository's Pages settings and enable the certificate. No page needs an edit: every path is relative, so the site moves from the project path to the subdomain root unchanged
