# noroshi

Signal-fire alerts for AI coding agents: parse a Claude Code hook payload, decide
by per-event policy, deliver to each configured backend. Rust, `substrate.rust.tool`.

| Module | Responsibility |
|--------|----------------|
| `config` | `noroshi.yaml`; per-entry resolution of policies and ntfy priorities (`Priority` is 1..=5) |
| `alert` | `Alert` from a payload: host, event, 8-char session label, nothing else |
| `sink` | `Sink` (Discord, Ntfy) builds an `HttpRequest`; `Transport` sends it (`Ureq` in production, a recorder in tests) |

Rules:

- `noroshi hook` always exits 0 and never writes stdout: a Stop hook that prints a
  block decision or exits 2 would keep the agent running.
- Secrets are file paths read at run time; no error message may echo a secret's contents.
- Tests never touch the network: unit tests use a recording `Transport`, CLI tests a
  local `TcpListener`.
- A `Cargo.lock` change ships with `gen build .` and the regenerated `Cargo.gen.lock`.
