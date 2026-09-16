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
| `entrypoint_unix` | `sh -c "if [ $(uname) = Darwin ]; then B=./bebok-index-macos; else B=./bebok-index; fi; chmod +x $B && exec $B --plugin-server"` |
| `prompt_file` | `AGENT_INDEX.md` |

`prompt_file` names the prompt file in the slot: the engine reads
`AGENT_INDEX.md` out of the slot dir and injects it as the plugin's agent
prompt. When the file (or the field) is missing the engine falls back to its
built-in prompt, so old slots keep working.

## Release assets

Every `vX.Y.Z` tag is built and published by
[`.github/workflows/release.yml`](./.github/workflows/release.yml):
three build legs (`windows-latest`, `ubuntu-latest`, `macos-latest`)
compile the binary and upload it as a workflow artifact; the `assemble` job
(`ubuntu-latest`) merges them into ONE universal asset plus a `checksums.txt`
and publishes the GitHub release.

| asset | contents |
| --- | --- |
| `bebok-index-X.Y.Z.zip` | `bebok-index.exe` (Windows), `bebok-index` (Linux), `bebok-index-macos` (macOS), `bebok-plugin.json`, `AGENT_INDEX.md` — all at the **archive root** (no wrapping folder) |
| `checksums.txt` | `<sha256>  bebok-index-X.Y.Z.zip`; the digest goes into the registry as `asset_sha256` |

One universal `.zip` because the engine registry carries a single `asset_url`
per plugin (no per-OS templating — the installer downloads the URL verbatim),
so every platform binary must ship in one package. The layout rule is the
engine's, which detects the format from the URL suffix only: a `.zip` must
keep every file at the archive root (a wrapping directory would bury the
manifest). The downloader requires `https`, verifies SHA-256 (64 hex chars),
caps the transfer at 256 MiB / 60 s, and rejects symlinks and absolute or `..`
entries.

Release prerequisites:

- The tag version must equal `version` in `Cargo.toml` **and** in
  `bebok-plugin.json`; the workflow fails before building otherwise.
- Each leg packages `target/release/bebok-index[.exe]` and fails with a clear
  message when `cargo build --release --bin bebok-index` produced no
  executable — the crate must keep the `bebok-index` binary target.
- Publishing uses the preinstalled `gh` CLI with the workflow `GITHUB_TOKEN`;
  the `assemble` job creates the release, re-runs upload into it with
  `--clobber`. Only first-party actions are used (`checkout`,
  `upload-artifact`, `download-artifact`) plus `dtolnay/rust-toolchain@stable`.

### Why the Unix entrypoint is a `sh -c` shim

The engine unpacks archives **without preserving file modes** (zip external
attributes are ignored, tar entries are extracted with default permissions), so
after an asset install the Unix slot has a binary with no exec bit. A bare
`./bebok-index` entrypoint would therefore fail with a permission error, even
though the same manifest works after a `git clone` in an environment where the
file kept its mode.

The shim sidesteps that: `/bin/sh -c "if [ $(uname) = Darwin ]; then
B=./bebok-index-macos; else B=./bebok-index; fi; chmod +x $B && exec $B
--plugin-server"` runs in the slot dir (the engine's cwd), picks the binary
matching the OS (the universal zip carries `bebok-index` for Linux and
`bebok-index-macos` for macOS side by side), makes it executable first, and
`exec` replaces the shell so the JSON-lines stdio talks to the plugin
directly and no wrapper process lingers. The engine's entrypoint parser
honours quotes, so the whole `-c` argument stays a single argv entry.

Once the engine preserves exec bits on unpack, `entrypoint_unix` can be dropped
(or simplified to `bebok-index --plugin-server`, which is exactly the generic
`entrypoint` already in the manifest).
