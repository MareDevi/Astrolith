# Astro compatibility fixtures

`mise run test:fixtures` installs committed frozen Bun locks into temporary copies and runs each project's actual Astro sync. A fixture-only wrapper delegates to the real `glob()` loader and records `store.set` IDs and source paths. The task then runs the production Rust discovery/index helper against that same installation and compares complete mappings. It also invokes the headless GPUI workflow on both installed projects, checking picker selection, keyboard edits, MDX source, retained tabs and unchanged disk contents.

Verified pairs are Astro 5.18.2 / MDX 4.3.14 and Astro 7.4.4 / MDX 8.1.2. The regular `workspace` config is the supported static subset. The separate `oracle` config includes a custom-ID collection to demonstrate that logical IDs cannot be used to reconstruct source paths; production rejects that option. `unsupported/legacy` is diagnostic-only.

Every subprocess has a 60-second deadline and a 1 MiB output cap. Generated oracle JSON and a Markdown report, including installed source hashes, go to the OS temporary directory. `mise run test:fixtures -- --keep-temp` retains the temporary projects for inspection. Normal runs remove them. No selected user project is executed by discovery.
