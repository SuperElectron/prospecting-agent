# Prospecting Agent

Finds your next customers, researches them, writes the email, sends it, and
handles the reply — every morning, before you sit down.

It runs on the machine on your desk. A Mac mini is enough; a DGX Spark is
plenty. Your own model does the writing, so there's no seat license and no
per-lead token bill, and your pipeline never leaves the building.

What it plugs into:

- **Apollo** — finds contacts and companies, fills in what's missing
- **Tavily** — reads up on the account and catches buying signals
- **Your local LLM** — writes every email (OpenAI-compatible; gpt-oss-120B here)
- **Gmail or SendGrid** — sends it, then watches for the reply
- **HubSpot or a CSV folder** — bring a CRM, or don't

All you have to do is bring your API keys, fill in `.env`, and run it!

---

# Table of contents

1. [A day of it](#a-day-of-it)
2. [Quickstart](#quickstart)
3. [Integrations](#integrations)
4. [Docs](#docs)
5. [License](#license)


<a name="a-day-of-it"></a>
## A day of it

---

The worker keeps its own schedule. You do not start anything by hand.

| Time (UTC) | What happens |
|---|---|
| 06:00 | New contacts in `data/` are imported |
| 06:30 / 06:40 | Contacts and companies enriched |
| 06:45 | New contacts discovered at target accounts |
| 07:00 | Companies researched |
| 07:15 | Contacts enrolled in an outreach sequence |
| 08:00 | Buying signals detected |
| hourly | Email sent — only inside your cadence windows |
| hourly | Replies read and classified, follow-up tasks run |
| 16:30 | Daily digest, after the send window closes |
| Mon 09:00 | Weekly report |

Nothing is sent until you say so — `DRY_RUN=true` is the default. Apollo spend
sits under a daily credit cap you set, with a per-run budget on top of it.

<a name="quickstart"></a>
## Quickstart

---

```sh
./scripts/setup.sh                              # deps, services, build, tests
# fill in your API keys in .env
prospecting-agent gmail-auth --daily-limit 25   # authorize a sending account
prospecting-agent health                        # db, LLM, memory, gmail, capacity
prospecting-agent worker                        # run it
```

`prospecting-agent serve` adds a local control API for running a job now,
checking a contact, or posting an inbound reply. See
[Operations](docs/operations.md).

<a name="integrations"></a>
## Integrations

---

| Integration | Role | Required |
|---|---|---|
| [Apollo](https://apollo.io) | Contact and company discovery + enrichment | Yes |
| [Tavily](https://tavily.com) | Web research and buying-signal detection | Yes |
| Local LLM (OpenAI-compatible) | All generation and analysis (e.g. gpt-oss-120B) | Yes |
| [Mem0](https://mem0.ai) (self-hosted) | Semantic memory over research and interactions | Yes (bundled in compose) |
| Postgres | System of record + job queues | Yes (bundled in compose) |
| Gmail | Email sending + reply monitoring | One email provider |
| [SendGrid](https://sendgrid.com) | Email sending (alternative to Gmail) | One email provider |
| [HubSpot](https://hubspot.com) | CRM sync + engagement logging | Optional (CSV works without) |
| CSV import | No-CRM contact source | Optional |
| Slack | Rep notifications, digests, error alerts | Planned (M6) |
| [HeyReach](https://heyreach.io) | LinkedIn outreach | Optional (off by default) |

<a name="docs"></a>
## Docs

---

- [Setup](docs/setup.md) — fresh clone to green gates
- [Architecture](docs/architecture.md) — layers, invariants, queue isolation ([diagram](docs/architecture.svg))
- [Operations](docs/operations.md) — schedules, control API, spend controls, failure playbook
- [Contributing](CONTRIBUTING.md)
- [Wiki](https://github.com/SuperElectron/prospecting-agent/wiki) — plan of record

<a name="license"></a>
## License

---

MIT: [LICENSE](LICENSE).
