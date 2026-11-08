# Assets and licences

The site's own code is MIT, as `LICENSE` says. The files under `assets/vendor/ddlc/` come from [ddlc-themes](https://github.com/rokokol/ddlc-themes) and keep their own terms; the fonts are not covered by the MIT licence at all

## Vendored from ddlc-themes

| File | What | Licence |
| --- | --- | --- |
| `ddlc-tokens.css` | the DDLC roles as custom properties, with no selector of an application | MIT, as the repository that generates it |
| `ddlc-ui.css` | the pixel kit: `.bar`, `.work`, `.card`, `.code`, `dialog`, `.cloud` and the rest | MIT |
| `ddlc-report.css` | the prose sheet: the taped quote, the kitty code window and the "Just Monika." pop-up | MIT |
| `ddlc-theme.js` | the three-state theme button | MIT |
| `ddlc-cloud.js` | the colour probe, the ordered dithering and `startCloud` | MIT |

The colours in those files are Team Salvato's, measured off [ddlc.moe](https://ddlc.moe/) by [ddlc-palette](https://github.com/rokokol/ddlc-palette). The mapping of colour to role is the work of ddlc-themes. This project is unaffiliated with and not endorsed by Team Salvato

## Fonts

| File | Font | Author | Licence |
| --- | --- | --- | --- |
| `DepartureMono-Regular.woff2` | [Departure Mono](https://github.com/rektdeckard/departure-mono) | Helena Zhang | SIL OFL 1.1, in `DepartureMono-LICENSE.txt` |
| `MyaNunito.woff2` | Mya Nunito, a Modified Version of Nunito, Noto Sans and Noto Sans Math | The Nunito Project Authors and the Noto Project Authors | SIL OFL 1.1, in `MyaNunito-LICENSE.txt` |
| `MyaNunito-Italic.woff2` | the same face in italic | The Nunito Project Authors and the Noto Project Authors | SIL OFL 1.1, in `MyaNunito-LICENSE.txt` |

The two licence texts sit beside the fonts in `assets/vendor/ddlc/` and are unmodified

`Doki` is named by the kit as the heading face and is not shipped here: it is free for personal use only, so a reader who has it installed sees it and everyone else sees Mya Nunito Black

## The project's own images

`assets/logo.svg` and `assets/icon-github.svg` are the site's own SVG, under the repository's MIT licence. `logo.svg` is also the favicon: one mark, one file, and `404.html` carries the same mark inline because that page is served at an unknown depth

The screenshots on the home page are not in `site/`: `docs/img/` arrives in the published artifact through `.github/workflows/pages.yml`, so the pages use `./docs/img/...` from the site root without a second binary copy in the repository
