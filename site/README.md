# GitHub Pages site

Static landing page for BOZKURT CODE, published at <https://cumabozkurt.github.io/bozkurt-code/>.

- `index.html`, `styles.css`, `main.js`: plain HTML/CSS/JS, no build step, bilingual (EN/TR toggle, `?lang=tr`).
- Screenshots and the logo are **not duplicated** here. `.github/workflows/pages.yml` copies
  `docs/screenshots/*.png` to `screenshots/` and `apps/desktop/public/logo.svg` to `logo.svg` when it builds the site.

Preview locally:

```sh
rm -rf /tmp/bz-site && cp -r site /tmp/bz-site && mkdir -p /tmp/bz-site/screenshots \
  && cp docs/screenshots/*.png /tmp/bz-site/screenshots/ && cp apps/desktop/public/logo.svg /tmp/bz-site/ \
  && python3 -m http.server -d /tmp/bz-site 8000
```
