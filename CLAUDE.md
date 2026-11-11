# noroshi

Signal-fire alerts for AI coding agents: parse a Claude Code hook payload, decide
by per-event policy, deliver to each configured backend. Rust, `substrate.rust.tool`.

Module map (`config`, `alert`, `verdict`, `sink`): [README.md#modules](./README.md#modules), read it before you move a responsibility between modules.

Rules:

- `noroshi hook` always exits 0 and never writes stdout: a Stop hook that prints a
  block decision or exits 2 would keep the agent running.
- Secrets are file paths read at run time; no error message may echo a secret's contents.
- The Discord backend posts only to a `webhook_hosts` entry (default: Discord). A dotfile
  layer in the working tree cannot change that list: repo content must not widen it.
- Tests never touch the network: unit tests use a recording `Transport`, CLI tests a
  local `TcpListener`.
- A `Cargo.lock` change ships with `gen build .` and the regenerated `Cargo.gen.lock`.
