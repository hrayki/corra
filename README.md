# Corra

<img src="apps/app/logo/mark.svg" width="56" alt="">

A very personal agent development environment, built around the features I use. It's public in case someone works
the way I do and finds it useful.

I recommend you **don't** use it. There's no support promise: I keep it up to date as I use it and add what I want.
You can open an issue or a pull request, but I'll likely pass on anything I don't need myself.

Corra is a hub in the cloud and an app for the desktop, Android and the browser. In the app I pick repos from my
GitHub, and the hub starts a machine on one of my cloud providers, clones the repos onto it, sets it up from my setup
repo and runs Claude Code there. Corra isn't a harness itself.

Corra drives Claude Code through interfaces that aren't documented, so any Claude Code update can break it.

The hub holds the keys for GitHub, the machine providers, backups and the Android test phone, and does the GitHub work
agents need as tools: pull requests, checks, issues and merging. Machines reach GitHub only through the hub. A machine
does hold some keys: the Claude Code token, the env vars its project uses (make those test keys with spending caps),
and its own hub token, which sits in each repo's `.git/config`. A manager project is the exception: its
machine gets the hub's own keys and every repo. Pushing to the default branch or a tag, and merging, wait for my yes
unless the project allows them.

Machines sleep when they're idle, each provider in its own way, with their whole disk kept, and wake with the next
message.

Corra was built by AI with close to no supervision, and so were most of these docs.

## Running the hub

The hub is one container, built from the `Dockerfile`. Wherever it runs, it needs:

- a health check on `/healthz`;
- somewhere for its data: an S3-compatible bucket (below), or else a volume at `/data`;
- for a deploy without downtime, the new container started and answering `/healthz` while the old one still runs, then
  SIGTERM to the old one with 30 seconds to finish. With a bucket, the new hub waits until the old one has copied its
  last changes and handed over.

Set:

- `CORRA_PUBLIC_URL`: its https address.
- `CORRA_ACCESS_KEY`: 16 or more characters you sign in with everywhere. Changing it signs everything out.
- `CORRA_MASTER_KEY`: from `openssl rand -base64 32`. It encrypts the keys the hub keeps and isn't in its backups.
  Never change it.
- `CORRA_STORAGE_ENDPOINT`, `CORRA_STORAGE_BUCKET`, `CORRA_STORAGE_ACCESS_KEY` and `CORRA_STORAGE_SECRET_KEY`: the
  bucket that holds the hub's data, so it needs no volume. `CORRA_STORAGE_REGION` (default `auto`) and
  `CORRA_STORAGE_PREFIX` (default `corra`) are optional.

And if you need them:

- `CORRA_LISTEN`: the address to listen on. Default `0.0.0.0:$PORT` when `PORT` is set, otherwise `127.0.0.1:7810`.
- `CORRA_DATA`: the data folder. Default `./data`; the image uses `/data`.
- `CORRA_IDLE_MINUTES`: how long an idle machine stays awake. Default 15.
- `CORRA_PHONE_REGION`, `CORRA_PHONE_IMAGE` and `CORRA_PHONE_NAME`: the test phone's DigitalOcean region (default
  `sfo3`), image snapshot name and droplet name.
- `CORRA_RUNNER_BIN`, `CORRA_MODAL_HELPER` and `CORRA_LITESTREAM`: the helper binaries, when they aren't beside the
  hub's.
- `CORRA_DEV_NO_AUTH`: skips signing in. Only on a loopback address, for working on corra.

Open the address, sign in with the key, and in Settings add:

- `claude.token` (from `claude setup-token`), or `anthropic.api_key`.
- `github.token`: fine-grained, for your repos, with Contents, Pull requests and Issues read and write, Actions and
  Checks read, and no Workflows.
- `modal.token_id` and `modal.token_secret`, or `agent37.key`, for machines. `digitalocean.token` adds the Android test
  phone.
- `railway.token`, `cloudflare.token` and `cloudflare.account_id` if a manager project needs them; only those
  projects get them.
- Your setup repo, the env vars your projects need, a bucket for backups, and how long to keep hidden threads.
- Notifications, in each browser that should get them.

## The apps

The desktop app needs Claude Code, signed in, and builds on Linux with Rust, pnpm and Tauri's [system
packages](https://tauri.app/start/prerequisites/):

    cd web && pnpm install && pnpm build
    cd ../apps/app && pnpm install && pnpm tauri build

On first start it asks for the hub's address and key, or a pairing code, and adds the laptop to the hub as a machine.
While the hub can't be reached, it carries on with a built-in hub and hands those threads to the hub when it's back. It
keeps its own data in `CORRA_LOCAL_DATA`, by default `~/.local/share/corra`.

The Android app is `pnpm tauri android build --apk`, signed with the `keystore.properties` file that
`CORRA_ANDROID_SIGNING` points to. It signs in with the key, or with a pairing code from the hub's Settings, whose QR
code holds the address too.

## A machine of your own

Any Linux computer can be a machine: `corra-hub machine add NAME --token-file FILE` on the hub, then run
`corra-runner` there with `CORRA_HUB` set to the hub's address and `CORRA_TOKEN_FILE` (or `CORRA_TOKEN`) to its
token. The runner also reads `CORRA_PARK_MINUTES` (how long an idle Claude Code process is kept, default 30) and, where
the defaults don't fit, `CORRA_CLAUDE_BIN`, `CORRA_CHROMIUM` and `CORRA_ADB`.

## The setup repo

Any repo on your GitHub with a `corra/` folder. Machines get only that folder, and of its setups only their own
projects':

```
corra/
  shared/                  every machine
    AGENTS.md              Claude Code's rules
    skills/<name>/         its skills
    claude-settings.json   merged into its settings
    mcp.json               more MCP servers (optional)
    setup                  a script that runs first (optional)
  setups/<name>/           one kind of project
    setup                  what I'd otherwise do by hand on its machine
    repos                  its repos, one owner/name a line
    env                    the env vars it needs, one name a line
    AGENTS.md              rules its repos can't hold, like private ones for a public repo (optional)
```

A setup runs on a project's first machine and again after it changes, so it has to be safe to run twice. It gets
`CORRA_PROJECT` and `CORRA_PROJECT_DIR`. A new project picks the setup that lists its repo, or is named like it.

## Working on corra

    scripts/build                                       # everything, as static binaries
    cargo run -p corra-hub -- serve                     # a hub on 127.0.0.1:7810
    cargo run -p corra-local                            # the desktop app's core, without its window
    cd web && pnpm dev                                  # the app on :7820, against the hub at CORRA_HUB
    cargo test --workspace                              # the tests
    rm -rf web/src/proto && cargo test -p corra-proto   # the app's types, after changing the protocol

While the folder `~/.corra/recordings/` exists on a machine, the runner records each Claude Code process it starts
there: the app's commands, what the runner wrote and what Claude Code printed. The runner's tests replay the recordings
kept in `crates/corra-runner/tests/replays/`. `cargo test -p corra-runner record_ -- --ignored` records them again, with
`CORRA_RECORD_TOKEN_FILE` pointing at a Claude token and `HOME` at an empty folder so your own settings stay out of
them.

[DESIGN.md](DESIGN.md) says how it fits together, and [SECURITY.md](SECURITY.md) what it protects. MIT licensed.
