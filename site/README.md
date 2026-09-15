# site/

This is the project page, served at
[ozd.metebalci.com](https://ozd.metebalci.com) by GitHub Pages. Its files
are written by hand, with no build step and no generator.

- `index.html` is the page. Its drawings are inline SVG, defined once at
  the top of the file and placed with `<use>`. CADR's body, face and waving
  arm are Cold Boot's own parts, from the manga-style zine about the CADR
  by the same author. OZ and muir-fpga's board are drawn for this page in
  the same hand. The
  switch diagram is inline SVG as well, drawn in its own viewBox units so
  that it scales with its box.
- `style.css` is the stylesheet. It has ink and paper with one spot color,
  the size tokens that every `font-size` comes from, and no dark mode.
- `fonts/` holds the three families, served from here rather than from
  Google. `fonts/README.md` says where each came from, which characters it
  was cut down to, and under what license.

`.github/workflows/pages.yml` publishes this directory on every push to
`main` that touches it. The repository's Pages source is GitHub Actions,
and its custom domain is `ozd.metebalci.com`.

To look at the page before pushing, open `index.html` in a browser, or
serve the directory:

    python3 -m http.server -d site 8000

The fonts hold only the characters the page draws. A character outside
that set is drawn in a fallback face, so adding one means cutting the
fonts again, as `fonts/README.md` describes.
