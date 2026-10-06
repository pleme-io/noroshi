# noroshi (狼煙)

Signal-fire alerts for AI coding agents. noroshi reads a Claude Code hook
payload on stdin, decides whether the event is worth the operator's attention,
and delivers it to every configured backend: an [ntfy](https://ntfy.sh) topic
(phone push), a Discord webhook, or both.

guardrail is what the agent must not do; noroshi is what the operator must be told.

## CLI

```bash
noroshi hook                   # Claude Code hook entrypoint; always exits 0, prints nothing
noroshi check                  # report the config and whether each backend can deliver
noroshi test [--event Stop]    # send one test alert through every backend
```

`--config <path>` overrides `$XDG_CONFIG_HOME/noroshi/noroshi.yaml`.

## Configuration

```yaml
host: workstation
events:                         # per event: signal | silent; unlisted = silent
  Stop: signal
  Notification: signal
  SubagentStop: silent
  SessionEnd: silent
ntfy:
  server: https://ntfy.sh       # default
  topic_file: /path/to/topic    # read at run time; the topic is a secret
  priorities: { Stop: 5, Notification: 4 }   # 1..5, default 3
  tags: { Stop: [white_check_mark] }
  titles: { Stop: "Done" }
webhook_url_file: /path/to/discord-webhook   # optional Discord backend
mention: "<@123>"                            # optional Discord mention
```

Secrets are never values in the config, only paths read when an alert fires, so
a topic or webhook never lands in a Nix store path. ntfy is published as JSON to
the server root, so the topic stays out of the URL.

An alert carries the host, the event and the first eight characters of the
session id. Nothing else from the payload (prompts, messages, paths, code) is
sent anywhere.

A bad entry (an unknown policy, a priority outside 1..5) is ignored on its own
and reported by `noroshi check`; one backend failing never stops another.

## Wiring

In the pleme-io fleet, blackmatter-claude's `noroshi` options render the config
and register `noroshi hook` only on events set to `signal`.

## License

MIT
