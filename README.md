# prefixe

Prepend validated prefixes to shell commands, with support for confirmed mappings and
speculative candidate learning.

Designed as the core library for [`coursers`](https://github.com/89jobrien/coursers) /
`rx` prefix config integration.

## What it does

- Reads a TOML prefix config (`~/.config/rx/prefixes.toml` by default)
- Rewrites shell command strings by prepending a known prefix to each segment
- Supports compound commands split on `&&`, `||`, `;`, `|`
- Tracks speculative "candidate" probes for learning new mappings on success
- Exposes `PrefixStore` and `ProbeStore` traits for easy testing and substitution

## Usage

```rust
use prefixe::{FilePrefixStore, PrefixStore, rewrite_command};

let store = FilePrefixStore { path: FilePrefixStore::default_path() };
let config = store.load();
let result = rewrite_command("gh issue list && gh pr list", &config);
println!("{}", result.rewritten);
// → "op plugin run -- gh issue list && op plugin run -- gh pr list"
```

## Config format

`~/.config/rx/prefixes.toml`:

```toml
[mappings]
gh = ["op", "plugin", "run", "--"]
"cargo test" = ["dotenvx", "run", "--"]

[[candidate_prefixes]]
candidate_prefixes = [["op", "run", "--"]]

learn_on_successful_fallback = true
```

## Environment variables

| Variable          | Default                             | Purpose                     |
| ----------------- | ----------------------------------- | --------------------------- |
| `CRS_RX_PREFIXES` | `$XDG_CONFIG_HOME/rx/prefixes.toml` | Override prefix config path |
| `CRS_CTX_DIR`     | `.ctx`                              | Directory for probe state   |

## License

MIT OR Apache-2.0
