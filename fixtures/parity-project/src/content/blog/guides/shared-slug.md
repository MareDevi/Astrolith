---
title: Shared slug in guides
description: First entry that deliberately duplicates the slug value shared-duplicate.
slug: shared-duplicate
date: 2025-01-16
draft: false
order: 3
tags: [duplicate-slug, guides]
category: blog
status: published
author: ada
related: [markdown-showcase]
---

# Shared slug in guides

This entry declares `slug: shared-duplicate`, which Astro's glob loader uses as
its entry id. The docs and archive collections reuse the same slug value while
their collection ids keep the entries distinct.
