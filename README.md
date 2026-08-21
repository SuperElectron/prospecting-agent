# Prospecting Agent

An open-source AI prospecting agent that finds leads, researches them, and runs personalized multi-channel outreach on autopilot. 
- One Rust binary + Postgres + Mem0
- Everything runs locally, deployable anywhere.
- Bring your own CRM (HubSpot or plain CSV) and your own API keys. 
- All LLM work runs against a local OpenAI-compatible endpoint.

**Built with a DGX Spark running gpt-oss-120B.**

## Docs

- Read the [wiki](https://github.com/SuperElectron/prospecting-agent/wiki) for details. 
- Setup guides in `docs/`.
- Read [settings.example.json](.claude/settings.example.json) for how to plug in your API keys.

## License

MIT: [LICENSE](LICENSE).
