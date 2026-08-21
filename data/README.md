# Seed data

Sample dataset for the CSV import path. Lets the agent dry-run with zero external APIs.

- `companies.csv` — real Austin, TX startups. Domains, industries, and approximate size classes from public information.
- `contacts.csv` — **fictional people**. Names are invented, emails are role-pattern placeholders. Any resemblance to real employees is coincidental. Replace with your own list before sending anything.
- `notes.csv` — sample research notes keyed by company domain.

Schema is the contract for `workflows::sync::csv`. Column order matters; keep headers as-is.
