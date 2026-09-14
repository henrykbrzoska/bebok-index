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
