# operate — drive the prospecting agent

Use the control API (127.0.0.1:8088 when `serve` is running) or the CLI.

## Check state

- `prospecting-agent health` — nonzero exit means something is down; the JSON
  names the failing check.
- `GET /control/report` — 7-day counts.
- `GET /control/contacts/{email}` — full picture of one contact.

## Run work

- Inline: `prospecting-agent job <name>` (names from `GET /control/jobs`).
- Queued: `POST /control/jobs/{name}/enqueue` (needs a running worker).
- Never bypass the jobs layer to call workflows directly; the jobs layer owns
  credit budgets and timeouts.

## Interpreting job reports

Every job returns a JSON report of counters. Zeroes are normal outside the
send window. `search_failed`/`failures` rising with successes flat means an
upstream is down — check `health` first. `apollo daily credit cap limiting
this run` in the log means the shared ledger clamped a run; raise
`APOLLO_DAILY_CREDIT_CAP` deliberately, never reflexively.

## Safety rails you must not remove

- `DRY_RUN=false` is the only switch that sends real email.
- The control API stays on loopback; adding auth comes before any bind change.
- Opt-outs are terminal: never flip an `opted_out` contact back by hand.
