# `pages/fonts/`

These are the three families the page uses. They are carried here so that
reading the page asks nothing of a third party.

| file | face | used for |
|---|---|---|
| `dela-gothic-one.woff2` | Dela Gothic One | the display face: titles, numerals, the contents |
| `zen-maru-gothic-500.woff2` | Zen Maru Gothic Medium | the voice, which is what the page calls normal |
| `zen-maru-gothic-700.woff2` | Zen Maru Gothic Bold | the voice, emphasized, and the speech bubbles |
| `ibm-plex-mono-400.woff2` | IBM Plex Mono Regular | code, commands and the who-line |
| `ibm-plex-mono-500.woff2` | IBM Plex Mono Medium | the keys and the table headings |
| `ibm-plex-mono-600.woff2` | IBM Plex Mono SemiBold | the flags |

## Where they came from

All six were cut from the upstream TTFs in
[google/fonts](https://github.com/google/fonts) on 15 September 2026:
`ofl/delagothicone/DelaGothicOne-Regular.ttf`,
`ofl/zenmarugothic/ZenMaruGothic-Medium.ttf` and `ZenMaruGothic-Bold.ttf`,
and `ofl/ibmplexmono/IBMPlexMono-Regular.ttf`, `IBMPlexMono-Medium.ttf` and
`IBMPlexMono-SemiBold.ttf`. Two of the families are Japanese, and the whole
of one is megabytes. So each file keeps only the code points the page
draws: printable ASCII, `U+00A0` `U+00A9` `U+00B7` `U+2014` `U+2019`, and
the four katakana of オズ and コンコン, `U+30AA` `U+30B3` `U+30BA`
`U+30F3`. Each file is under 10 KB.

    pip install fonttools brotli
    pyftsubset DelaGothicOne-Regular.ttf --flavor=woff2 \
        --layout-features+=vert,vrt2 --name-IDs='*' \
        --unicodes=U+0020-007E,U+00A0,U+00A9,U+00B7,U+2014,U+2019,U+30AA,U+30B3,U+30BA,U+30F3 \
        --output-file=dela-gothic-one.woff2

The other five are cut the same way. `--name-IDs='*'` keeps each font's
copyright and license in its own name table.

**A character outside that list is drawn in a fallback face**, which is a
different face. Adding one means cutting all six again, with the new code
point added to `--unicodes`.

## License

None of the three families is this project's work, and none is under its
license. All three are under the **SIL Open Font License, Version 1.1**,
which permits redistribution with or without modification. The files here
are subsets, and each keeps its copyright notice and license in its
metadata.

- Dela Gothic One, copyright 2020 The Dela Gothic Project Authors ---
  <https://github.com/syakuzen/DelaGothic>
- Zen Maru Gothic, copyright 2021 The Zen Maru Gothic Project Authors ---
  <https://github.com/googlefonts/zen-marugothic>
- IBM Plex Mono, copyright 2017 IBM Corp., with Reserved Font Name "Plex"
  --- <https://github.com/IBM/plex>
