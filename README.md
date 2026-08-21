# Prospecting Agent

An open-source AI prospecting agent that finds leads, researches them, and runs personalized multi-channel outreach on autopilot. 
- One Rust binary + Postgres + Mem0
- Everything runs locally, deployable anywhere.
- Bring your own CRM (HubSpot or plain CSV) and your own API keys. 
- All LLM work runs against a local OpenAI-compatible endpoint.

**Built with a DGX Spark running gpt-oss-120B.**

## Integrations

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

## Docs

- [Setup](docs/setup.md) — fresh clone to green gates
- [Architecture](docs/architecture.md) — layers, invariants, queue isolation ([diagram](docs/architecture.svg))
- [Operations](docs/operations.md) — schedules, control API, spend controls, failure playbook
- [Contributing](CONTRIBUTING.md)
- [Wiki](https://github.com/SuperElectron/prospecting-agent/wiki) — plan of record
- Run `./scripts/setup.sh` on a fresh clone.

## Usage

```sh
prospecting-agent gmail-auth   # authorize a sending account (browser consent)
prospecting-agent health      # probe db, LLM, memory, gmail, capacity
prospecting-agent sync-csv    # import data/ into Postgres and memory
```
- Read [settings.example.json](.claude/settings.example.json) for how to plug in your API keys.

## License

MIT: [LICENSE](LICENSE).
