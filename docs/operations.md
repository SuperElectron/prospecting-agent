# Operations

## Run modes

```
prospecting-agent serve --port 8088    # control API (127.0.0.1 only)
prospecting-agent worker               # queue worker + cron schedules
prospecting-agent job <name>           # run one job inline
prospecting-agent job <name> --enqueue # queue it for the worker
prospecting-agent sync-csv [--dir d]   # one-off CSV import
prospecting-agent health               # readiness probe (nonzero on error)
prospecting-agent gmail-auth           # OAuth bootstrap
```

## Schedule (UTC)

| Time | Job |
|---|---|
| 06:00 | csv_sync |
| 06:30 | enrich_contacts |
| 06:40 | enrich_companies |
| 06:45 | discover_contacts |
| 07:00 | research_companies |
| 07:15 | outreach_sequence (enrollment) |
| 08:00 | detect_signals |
| hourly :10 | outreach_send (sends only inside cadence windows) |
| hourly :05/:35 | task_executor |
| hourly :20/:50 | reply_monitor |
| 16:30 | daily_digest (after the send window closes) |
| Mon 09:00 | weekly_report |

## Control API

All on 127.0.0.1; never expose without adding auth (tracked as a hard gate on
any bind change).

```
GET  /health                          # 503 if any check errors
GET  /control/jobs                    # job names + registered webhooks
POST /control/jobs/{name}/run         # run now (409 if already running, 504 past 600s)
POST /control/jobs/{name}/enqueue     # 202, queued for the worker
GET  /control/report                  # 7-day activity counts
GET  /control/contacts/{email}        # contact + engagements + sequence + strategy
POST /webhooks/inbound_reply          # {email, subject, body} -> reply outcome
```

## Spend controls

- `APOLLO_DAILY_CREDIT_CAP` (default 50) is the shared daily ceiling; each
  enrichment/discovery/task run reserves against the remainder and settles what
  it used (`apollo_spend` table, one row per day).
- Per-run budgets: enrichment 10, discovery 15, task executor 5.
- `DRY_RUN=true` (default) stops outbound email only.

## When something breaks

- Worker log line `job failed`/`timed out` fires the notifier; jobs retry via
  apalis.
- Stuck task claims reap after 1h; inbound messages give up permanently after
  3 attempts (`processed_inbound.attempts >= 3 AND completed_at IS NOT NULL`
  finds them).
- A run limited by the credit cap logs `apollo daily credit cap limiting this
  run` — one signal for all three spend paths.
