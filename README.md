# bebok-index

Per-instance code index for [Bebok](https://github.com/henrykbrzoska/bebok):
file scan, [tantivy](https://github.com/quickwit-oss/tantivy) full-text
search, background orchestrator and filesystem watcher.

Extracted from the Bebok monorepo (`engine/crates/bebok-code-index`).
The host application provides directory layout helpers (`code_index_dir`,
`ensure_code_index_dir`); this crate never touches the filesystem layout —
it only indexes what it is given.

## Use

```toml
[dependencies]
bebok-code-index = { git = "https://github.com/henrykbrzoska/bebok-index" }
```

```rust
let index = bebok_code_index::CodeIndex::open(&index_dir)?;
index.index_files(bebok_code_index::scan_project(&project_root)).await?;
let hits = index.search("query", 10)?;
```

Backends register via `BackendRegistry` (bundled: `tantivy`, `disabled`).

## License

GNU Affero General Public License v3.0 or later — see [LICENSE](./LICENSE).

## Install as a Bebok plugin

The engine installs bebok-index into a plugin slot
(`<project>/.bebok/plugins/bebok-index/`, declaration
`<project>/.bebok/plugins/bebok-index.json`) and runs it as a subprocess:
JSON-lines over the plugin's stdin/stdout, working directory = the slot dir.

- **Settings → General → Plugins → Install** (`POST /plugins/bebok-index/install`).
  The engine resolves the release through its plugin registry: when the registry
  entry carries `asset_url` + `asset_sha256` it downloads the matching archive,
  verifies the SHA-256 and unpacks it; otherwise it falls back to
  `git clone` of the repository at its latest tag.
- **Update** (`POST /plugins/bebok-index/update`) deletes the slot and installs
  it again, so a wiped or half-unpacked **binary is repaired** — no config edit
  and no manual download needed.
- **The git-clone fallback needs a Rust toolchain.** The repository ships
  sources only, so the clone path needs `cargo build --release` on that machine
  and the resulting `bebok-index` / `bebok-index.exe` must end up next to
  `bebok-plugin.json` in the slot dir. An asset install needs no toolchain at
  all.

The slot manifest (`bebok-plugin.json`) carries the entrypoints:

| field | value |
| --- | --- |
| `entrypoint` | `bebok-index --plugin-server` |
| `entrypoint_windows` | `bebok-index.exe --plugin-server` |
| `entrypoint_unix` | `sh -c "chmod +x ./bebok-index && exec ./bebok-index --plugin-server"` |
| `prompt_file` | `AGENT_INDEX.md` |

`prompt_file` names the prompt file in the slot: the engine reads
`AGENT_INDEX.md` out of the slot dir and injects it as the plugin's agent
prompt. When the file (or the field) is missing the engine falls back to its
built-in prompt, so old slots keep working.

## Release assets

Every `vX.Y.Z` tag is built and published by
[`.github/workflows/release.yml`](./.github/workflows/release.yml)
(matrix: `windows-latest`, `ubuntu-latest`, `macos-latest`). Each leg uploads
one archive plus a `checksums.txt`:

| asset | runner | contents |
| --- | --- | --- |
| `bebok-index-X.Y.Z-win-x64.zip` | windows-latest | `bebok-index.exe`, `bebok-plugin.json`, `AGENT_INDEX.md` at the **archive root** (no wrapping folder) |
| `bebok-index-X.Y.Z-linux-x64.tar.gz` | ubuntu-latest | exactly one top-level `bebok-index-X.Y.Z/` directory holding `bebok-index`, `bebok-plugin.json`, `AGENT_INDEX.md` |
| `bebok-index-X.Y.Z-macos-arm64.tar.gz` | macos-latest | same layout as Linux |
| `checksums.txt` | every leg | `<sha256>  <asset>` lines; the digest goes into the registry as `asset_sha256` |

The layout rules are the engine's, which detects the format from the URL suffix
only: a `.zip` must keep every file at the archive root (a wrapping directory
would bury the manifest), a `.tar.gz` must wrap everything in **exactly one**
top-level directory, which the engine strips on unpack. The downloader requires
`https`, verifies SHA-256 (64 hex chars), caps the transfer at 256 MiB / 60 s,
and rejects symlinks and absolute or `..` entries.

Release prerequisites:

- The tag version must equal `version` in `Cargo.toml` **and** in
  `bebok-plugin.json`; the workflow fails before building otherwise.
- The workflow packages `target/release/bebok-index[.exe]` (falling back to
  `bebok-code-index[.exe]`) and fails with a clear message when
  `cargo build --release` produced no executable — the crate must expose a
  binary target named `bebok-index` for a release to be publishable.
- Publishing uses the preinstalled `gh` CLI with the workflow `GITHUB_TOKEN`;
  the first matrix leg creates the release, later legs (and re-runs) upload into
  it with `--clobber`.

### Why the Unix entrypoint is a `sh -c` shim

The engine unpacks archives **without preserving file modes** (zip external
attributes are ignored, tar entries are extracted with default permissions), so
after an asset install the Unix slot has a binary with no exec bit. A bare
`./bebok-index` entrypoint would therefore fail with a permission error, even
though the same manifest works after a `git clone` in an environment where the
file kept its mode.

The shim sidesteps that: `/bin/sh -c "chmod +x ./bebok-index && exec
./bebok-index --plugin-server"` runs in the slot dir (the engine's cwd), makes
the binary executable first, and `exec` replaces the shell so the JSON-lines
stdio talks to the plugin directly and no wrapper process lingers. The engine's
entrypoint parser honours quotes, so the whole `-c` argument stays a single
argv entry.

Once the engine preserves exec bits on unpack, `entrypoint_unix` can be dropped
(or simplified to `bebok-index --plugin-server`, which is exactly the generic
`entrypoint` already in the manifest).
