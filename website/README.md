# Soroban DevKit — Website

Static landing page for `sdkt`. No framework, no build step, no backend, no
dependencies — three CSS files, one HTML file, ~40 lines of inline JS for the
"copy" buttons.

## Run locally

```bash
cd website
python3 -m http.server 8899
# open http://127.0.0.1:8899/
```

Or just open `website/index.html` in a browser — every asset is relative.

## Deploy

Any static host works (GitHub Pages, Cloudflare Pages, Netlify, Vercel):
publish directory = `website/`, build command = none.

## Content rules

Every claim, feature, and CLI command on the page is verified against this
repository:

- Commands come from `crates/sdkt-cli/src/main.rs` (clap definitions) and were
  each checked with `sdkt <cmd> --help` (exit 0).
- Terminal output blocks are copied from real local runs of
  `sdkt --version`, `sdkt wasm inspect`, `sdkt diff --upgrade-safety`, and
  `sdkt audit` against `crates/sdkt-cli/tests/fixtures/*.wasm`.
- The version shown is `v2.5.0`, matching `[workspace.package] version` in
  the root `Cargo.toml`.
- No adoption metrics, star counts, user counts, contributor counts,
  testimonials, partner logos, funding, or grant-approval claims appear
  anywhere on the page. Do not add them without verifiable evidence.

When bumping the release version, update the five `v2.5.0` occurrences in
`index.html` (nav pill, hero chip, hero terminal output, install note, footer).

## Files

| File | Purpose |
|------|---------|
| `index.html` | Whole page (hero, features, workflow, terminals, install, footer) |
| `base.css` | Tokens, reset, typography, nav, buttons, section shell |
| `sections.css` | Hero, terminal, feature cards, workflow grid |
| `layout.css` | Prose columns, install snippets, pills, CTA, footer, reduced-motion |
