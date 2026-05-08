# API Reference

Full rustdoc is available at <https://docs.rs/prefixe>.

## Core types

| Type              | Description                                               |
| ----------------- | --------------------------------------------------------- |
| `PrefixConfig`    | Pure domain config: mappings + candidates + learning flag |
| `OriginalCommand` | Newtype wrapper for a pre-rewrite shell command string    |
| `Segment`         | One shell segment plus its trailing separator             |
| `PrefixMatch`     | Lookup result: `Confirmed` or `Candidate`                 |
| `ProbeEntry`      | A pending candidate probe awaiting post-hook confirmation |
| `RewriteResult`   | Output of a rewrite: rewritten string + recorded probes   |
| `AuditState`      | Snapshot of mappings + probes for operator tooling        |

## Ports (traits)

| Trait             | Description                                         |
| ----------------- | --------------------------------------------------- |
| `PrefixStore`     | Read/write the prefix config (confirmed mappings)   |
| `ProbeStore`      | Read/write pending candidate probes                 |
| `CommandRewriter` | Strategy for rewriting a full command string        |
| `CommandSplitter` | Strategy for splitting/rejoining compound commands  |
| `PathResolver`    | Resolve filesystem paths for config and probe store |

## Adapters

| Type              | Description                                              |
| ----------------- | -------------------------------------------------------- |
| `FilePrefixStore` | `PrefixStore` backed by a TOML file                      |
| `FileProbeStore`  | `ProbeStore` backed by a TOML file                       |
| `EnvPathResolver` | `PathResolver` reading `CRS_RX_PREFIXES` / `CRS_CTX_DIR` |
| `TextualSplitter` | Default `CommandSplitter` (textual, not shell-grammar)   |
| `PrefixEngine`    | Composes `PrefixStore` + `ProbeStore` into the full API  |

## Free functions

| Function            | Description                                           |
| ------------------- | ----------------------------------------------------- |
| `split_segments`    | Split a command string on `&&`, `\|\|`, `;`, `\|`     |
| `rejoin`            | Reconstruct a command string from segments (lossless) |
| `lookup_prefix`     | Look up the prefix for a single segment               |
| `rewrite_command`   | Rewrite a command using a `PrefixConfig`              |
| `rewrite_via_store` | Rewrite a command using a `PrefixStore` port          |
| `audit_state`       | Build an `AuditState` from both stores                |
