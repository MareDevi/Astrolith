---
title: Configuration reference
description: Unknown frontmatter keys stay present even though the schema strips them.
date: 2025-03-03
draft: false
order: 2
tags: [configuration, reference]
category: reference
aliases: [/old/configuration, /legacy/config]
deprecatedSince: "1.0.0"
---

# Configuration reference

`aliases` and `deprecatedSince` are unknown to the schema. They must remain in
the complete source, be preserved by a patch, and never be reconstructed from
the normalized projection.

```json
{
  "preferences": {
    "autosaveDelaySeconds": 2,
    "copyedit": false
  }
}
```
