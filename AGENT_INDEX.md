# Agent index — read this before searching the codebase

Code index first — MANDATORY: whenever you need to find code (where is X
defined, who calls Y, which files match Z), FIRST query the local code
index instead of reaching for `grep` / `glob`:

- `code_index_status` — is the index for `<project-root>` ready? If it
  reports `indexing`, wait a moment and ask again; if `disabled`, fall
  back to `grep` / `glob`.
- `code_index_search` — full-text search over `<project-root>`. Prefer
  1–3 distinctive words (`query`) over long phrases; a file-name fragment
  also works. Keep `limit` small (default 5).

Only when the index is disabled or returns nothing useful, fall back to
`grep` / `glob` / `read_file` directly.
