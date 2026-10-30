# Notes

A local-first notes app with sync through storage you already own: a WebDAV folder or a plain directory. No account and no server of its own.

The repository holds the shared core so far. The apps for macOS and Android and the web interface are built on top of it in later stages.

## What is here

- `core/`: the Rust crate `notes-core`. Notes, folders, tags, flags, the Trash, attachments, saved filters and shared settings; the text of a note as an Automerge document; sync through a folder or WebDAV; attachments by SHA-256; storage encryption; push through ntfy; backups and the choice of side when a storage is connected. On top of that: the Markdown dialect of the editor with code highlighting and the edits the editor asks for, import of a Standard Notes backup, search, links between notes, version history and export to Markdown, HTML and JSON.
- `docs/architecture.md`: design decisions and why they were made.
- `docs/specs/`: the contract. `sync.md` covers data and sync, `product.md` the app, `editor.md` the editor, `import.md` the import from Standard Notes.

## Build and test

Needs Rust from `rust-toolchain.toml` (rustup picks it up) and, for `make deny`, [cargo-deny](https://github.com/EmbarkStudios/cargo-deny).

```sh
make test   # cargo test in core/
make lint   # rustfmt check and clippy with warnings as errors
make deny   # advisories, licenses and sources of the dependencies (deny.toml)
make fmt    # format the code
```

Tests are named after the requirement they check: `s3_*` checks S3 of `docs/specs/sync.md`, `r28_*` checks R28 of `docs/specs/product.md`, `e11_*` and `i7_*` check E11 of `docs/specs/editor.md` and I7 of `docs/specs/import.md`.

CI runs the same targets on every push and pull request.

## License

MIT, see `LICENSE`.
