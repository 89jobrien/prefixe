# prefixe

`prefixe` is a Rust library that prepends validated prefixes to shell commands.

It supports confirmed mappings (stored in a TOML config) and speculative candidate
learning for commands not yet explicitly mapped.

## Quick start

```toml
# Cargo.toml
[dependencies]
prefixe = "0.3"
```

```rust
use prefixe::{FilePrefixStore, PrefixStore, rewrite_command};

let store = FilePrefixStore::new(FilePrefixStore::default_path());
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

candidate_prefixes = [["op", "run", "--"]]

learn_on_successful_fallback = true
```

## Environment variables

| Variable          | Default                             | Purpose                     |
| ----------------- | ----------------------------------- | --------------------------- |
| `CRS_RX_PREFIXES` | `$XDG_CONFIG_HOME/rx/prefixes.toml` | Override prefix config path |
| `CRS_CTX_DIR`     | `.ctx`                              | Directory for probe state   |
