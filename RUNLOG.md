# Runlog

## 2026-03-18

- Documentation/CLI mismatch: certificate generation examples are using the wrong invocation form.
  - `rockbot` was used as if it were on `PATH`, but the actual local-build workflow in this repo is `./rockbot`.
  - `cert client generate` was documented/invoked with `--name gateway`, but the CLI actually expects a positional `<NAME>`.
  - Verified working form:
    - `./rockbot cert client generate gateway --role gateway --san 127.0.0.1 --san 172.30.200.146 --san localhost --days 784`

- CLI capability gap: `cert enroll create` does not currently allow multiple roles.
  - Attempted:
    - `./rockbot cert enroll create --role client --role tui --role agent --uses 1 --expires 24h`
  - Actual result:
    - `error: the argument '--role <ROLE>' cannot be used multiple times`
  - Desired future behavior:
    - allow multiple roles on enrollment token creation
    - update docs/help text accordingly once implemented
