# Privacy and local data

Astrolith is a local-first desktop application. It has **no telemetry**, no
analytics endpoint, no anonymous identifier, no event queue and no account
system. The local `telemetry_enabled` preference is retained consent state
only; nothing reads it to send data anywhere.

## Network use

In a normal session Astrolith itself makes no network request except one case:

- **Update check.** When a release build was compiled with both
  `ASTROLITH_UPDATE_MANIFEST_URL` (HTTPS) and `ASTROLITH_UPDATE_PUBLIC_KEY`, the
  application fetches that one signed manifest about two seconds after launch
  and whenever you choose **Check for updates…**. It verifies an Ed25519
  signature with the compiled-in key and a SHA-256 digest for the artifact, then
  offers an external download in your browser. It never replaces its own
  binary, never uploads the result, and sends no identifier or telemetry
  payload. As with any HTTPS request, the server sees ordinary connection
  metadata such as your IP address. A build without the URL and key has no
  update capability and makes no update request.

Everything else stays local. Opening a project, editing, saving, previewing and
checking for recovery are filesystem and process operations. Two qualifications:

- The project's own Astro tooling is started for schema generation and preview.
  That tooling is part of your project and may perform its own network access
  (for example dependency or font requests). Astrolith does not control it.
- **Open in browser**, **Open release page** and **Download** hand a URL to your
  default browser. The browser's network behavior is outside Astrolith.

## What is stored locally

Astrolith uses the XDG directories on Linux. The exact paths are also shown in
**Help → Privacy and local data**, which reads the same environment and renders
only paths, never file contents.

| Data | Default Linux path | Contents |
| --- | --- | --- |
| Preferences and project registry | `${XDG_CONFIG_HOME:-$HOME/.config}/astrolith/preferences/` | `global.json` (theme, layout and window geometry, heading colours, font size, autosave, default extension, IDE command, Copyedit, update skip version, local telemetry consent), `projects.json` (stable project IDs, canonical paths, package names, last-open times), and `projects/<project-id>.json` (per-project and per-collection preferences, including field mappings). A one-time migration retains the former `astrolith/preferences.json` as `preferences.v1.migrated.json` in the same directory. |
| Unsaved-edit recovery | `${XDG_CONFIG_HOME:-$HOME/.config}/astrolith/recovery/` | Recovery records keyed by canonical source. These **do** contain the unsaved document text plus its identity, revision and timestamp, which is what restores the content after a crash or unsuccessful save. |
| Save-failure artifacts | `${XDG_CONFIG_HOME:-$HOME/.config}/astrolith/save-failures/` | One record per save-failure episode: timestamp, error class, application version, build identity and project-relative source path. No buffer text, frontmatter values, snippets, environment variables or full project paths. |
| Logs | `${XDG_STATE_HOME:-$HOME/.local/state}/astrolith/logs/` | Daily JSON log files named `astrolith.YYYY-MM-DD.log`, seven retained by default. Logs include project roots, document paths, collection identifiers, revisions, PIDs, durations and bounded preview stderr excerpts (capped at 2,000 characters). They never include document content, preference bodies or environment variable values. |

Window geometry is stored with the global preferences. The preview panel's open
state and dock height are stored with the project preferences. Filters, sort,
search text and draft view state are session-only and are not written to disk.

### Overrides

These environment variables move the files and are useful for portable or test
runs:

- `ASTROLITH_SETTINGS_PATH` — absolute path of `global.json`; the preferences
  folder is its parent.
- `ASTROLITH_RECOVERY_DIR` — absolute recovery directory.
- `ASTROLITH_SAVE_FAILURE_DIR` — absolute save-failure directory.
- `ASTROLITH_LOG_DIR` — absolute log directory.
- `ASTROLITH_LOG`, `ASTROLITH_LOG_FILE`, `ASTROLITH_LOG_FORMAT`,
  `ASTROLITH_LOG_FILE_FORMAT`, `ASTROLITH_LOG_MAX_FILES` — logging behavior;
  see [logging](../technical/logging.md).

Every override must be an absolute path; an invalid value is ignored and the
default is used.

## Deleting local data

Quit Astrolith first, then delete the directories you want to remove:

```sh
# Preferences, project registry, recovery records and save-failure artifacts.
rm -rf "${XDG_CONFIG_HOME:-$HOME/.config}/astrolith"

# Logs.
rm -rf "${XDG_STATE_HOME:-$HOME/.local/state}/astrolith"
```

Deleting that data is not recoverable by Astrolith. If you use any of the
override variables above, delete those paths instead.

**Preferences → Advanced → Reset All** is narrower: it deletes only preferences
and the project registry, keeps recovery snapshots, save-failure records and
logs, and restarts the application with defaults. It says so in its
confirmation.

Recovery records and save-failure artifacts are never uploaded and are never
deleted automatically. Remove them with the commands above when you no longer
need them.
