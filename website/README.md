# Zap Website

Astro site, derived from the visual mockups under `design/`.

```bash
npm install
npm run dev      # http://localhost:4321
npm run build    # outputs dist/
```

Structure:

- `src/pages/index.astro` — Landing
- `src/pages/docs/[...slug].astro` — Docs dynamic route
- `src/content/docs/*.mdx` — Docs content (Content Collections)
- `src/components/` — Nav / Footer / Banner, etc.
- `src/styles/` — Design tokens and global styles
