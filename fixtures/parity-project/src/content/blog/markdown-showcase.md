---
title: Markdown showcase
description: Exercises code, links, images and every supported frontmatter family.
slug: markdown-showcase
date: 2025-01-08
updated: 2025-02-11T09:15:00Z
draft: false
featured: true
order: 1
rating: 4.5
tags: [astro, markdown, "code"]
category: blog
status: published
author: ada
related: [guides, notes/another-shared-slug]
cover: ./cover.png
gallery: ["./gallery-one.png", "./gallery-two.png"]
seo:
  title: Markdown showcase for Astrolith parity
  description: A stable source for writer, projection and schema checks.
  socialImage: ./cover.png
meta:
  views: 128
attributes:
  lang: en
  level: advanced
legacy: markdown
---

# Markdown showcase

This entry deliberately combines prose, code, links and images in one source.

## Inline code and links

Use `DocumentBuffer` for the complete source and `WriterProjection` for the
writer. Read the [Astro content collections guide](https://docs.astro.build/en/guides/content-collections/)
or jump to the [blog guide index](/blog/guides/).

![Parity banner](/parity-banner.svg)

## Fenced code

```rust
pub struct WriterProjection {
    pub visible: String,
    pub segments: Vec<ProjectionSegment>,
}
```

```ts
type FrontmatterValue = string | number | boolean | null;

export function isPatchable(value: FrontmatterValue): boolean {
  return value !== null;
}
```

## Tables and quotes

| Surface | Fixture state |
| --- | --- |
| Frontmatter | lossless |
| Body | complete source |
| Assets | local |

> The writer projection hides frontmatter without deleting it from the buffer.

<!-- The HTML comment is intentional source content for projection boundaries. -->
