# Security

Corra is a personal project: one hub per person, with no accounts or roles inside it.

## What it protects, and what it doesn't

- **Machines aren't trusted.** Their agents run whatever a model decides, on code and pages from anywhere. So the hub
  keeps the keys, and a machine's token lets it reach only its own threads, its project's repos and its project's
  setups.
- **What an agent can still do** is anything its machine can: read and change its project's repos, push any branch
  but the default one, delete any branch but the default one, open pull requests and issues, and use the Claude token
  and env vars it was given. A workflow that runs on any branch runs that branch's code with the repo's secrets, so
  keep secrets in workflows that run only on the default branch or tags.
- **What waits for a yes:** pushing to the default branch or a tag, and merging, unless the project allows them. A yes
  covers the exact commits it showed.
- **A manager project is trusted with everything:** its machine gets the hub's own keys, every repo, merging
  without asking, and the tools that manage corra. Only the app can turn it on.
- **Whoever has the access key has the hub.** The master key encrypts the keys the hub keeps, and isn't in its
  backups.

## Reporting a problem

Report it privately on GitHub, from this repository's Security tab ("Report a vulnerability"), not in a public issue.
There's no bounty and no promised response time, but I read every report.
