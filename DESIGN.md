# How corra works

The README says what corra is. This is how it fits together.

## The parts

- **Hub** (`crates/corra-hub`): one always-on service with an SQLite database. It keeps every thread and every event,
  holds the keys encrypted with the master key, makes machines and puts them to sleep, hands them the setup kit, carries
  out the agents' tools, serves the app, and relays everything between the apps and the machines. It never runs agent
  code. With a storage bucket set, it runs under Litestream: an empty disk is restored from the bucket and every change
  streams back to it, so the hub needs no volume. A new hub waits until the running one has copied its last changes and
  handed over a lease in the bucket, so two never write at once. Session files live in the database too. Its schema is a
  single base schema at version 12; a database older than that is refused rather than migrated.
- **Runner** (`crates/corra-runner`): on each machine. It connects out to the hub over one WebSocket, runs Claude Code
  through its stream-json interface, or Antigravity through `agy`'s Agent Client Protocol server, and serves the
  thread's panel: changes, files, terminal, browser, phone. When the hub serves a newer runner, a runner switches to it
  once nothing has needed it for two minutes: no turn running and no terminal open, except the desktop app's, which
  comes with the app. On Linux it starts as a small parent that takes in and reaps the processes orphaned below it,
  since a machine's PID 1 may not (Modal's doesn't).
- **Desktop core** (`crates/corra-local`): inside the desktop app. It adds the laptop to the hub as one of its machines
  and runs its runner. It also holds a built-in hub, the same code as the real one: when the hub can't be reached for a
  minute, the window moves to it, and when the hub is back, the threads started there move to the hub with their events
  and session files, and the window follows.
- **App** (`web`, SolidJS): served by the hub, and by the built-in hub while the hub is down. `apps/app` is its Tauri
  shell for desktop and Android. The hub tells the app which build it serves; an app on another build saves its drafts
  and reloads, and so does one that can't load a part after a deploy.
- `crates/corra-proto` holds the types everything shares (the app's are generated from them), `helpers/modal` a small
  Go helper for Modal, whose SDK has no Rust version, and `machines` the base image and the phone's setup script.

## Machines

A project has one machine, with its repos in `~/projects/<project>/<repo>`. Each provider describes itself in code: what
it is, its keys, and the kinds of machine it makes, each with its sizes, how it sleeps, any time limit and whether it
has KVM. The app and the manager's tools read those descriptions, so nothing outside a provider's own file names it. A
project picks a provider, a kind and a size, on one of:

- **Modal** (VM mode): asks for a quarter of a core and 1 GB and bursts up to the project's size, so it's billed on
  what it uses. Asleep, it's a snapshot of its disk. A machine lives 24 hours at most, so one still awake is put to
  sleep before then, even mid-turn. Modal writes a machine's DNS servers into `/etc/resolv.conf` on every boot and
  can't write through a link, so before a Modal machine sleeps its helper turns that file back into a plain one.
- **Agent37**: a VM, which has KVM, or a container, which is cheaper and doesn't. Both are billed on their size while
  awake. Asleep, it's stopped and only its disk is billed.
- **DigitalOcean** (droplet, with KVM): billed by the hour while it exists. The first machine waits while corra builds a
  base snapshot from the base image's Dockerfile. Asleep, it's a snapshot: the droplet is shut down, snapshotted and
  deleted, since a stopped droplet is still billed. It wakes as a new droplet from that snapshot in the same region
  (`digitalocean.region` in Settings, `sfo3` by default).
- **A machine paired by hand**: any Linux computer running the runner. The hub never starts or stops it.

The runner reports in its machine info whether `/dev/kvm` opens.

The first thread makes the machine: the runner downloads itself from the hub, clones the repos through it and applies
the setup kit. After 15 idle minutes, or an hour while an app shows the thread or a question waits, the hub puts it to
sleep. The next message wakes it as it was: dependencies, caches, files outside git; running programs aren't kept. A
machine that dies comes back with the next message: from its last sleep on Modal, as it was on Agent37, fresh if it
never slept. Resetting from the project's page deletes the machine and its disk, and the next thread starts fresh.
Every ten minutes the hub deletes any machine on its providers that it doesn't know.

## The setup kit

The hub downloads the setup repo from GitHub, keeps its `corra/` folder and serves it to machines at `/runner/kit`.
The runner copies `shared/AGENTS.md` to `~/.claude/CLAUDE.md`, links the skills, merges the settings, then runs
`shared/setup` and the project's setup with its env vars, `CORRA_PROJECT` and `CORRA_PROJECT_DIR`. At a wake it runs
them again only if the kit or the project's setup changed. If a setup fails, the next message shows the end of its log.
Each machine gets `shared/` and only the setups of the projects it serves, so one project's `AGENTS.md` stays off other
projects' machines.

The project's folder gets an `AGENTS.md` of corra's: the list of its repos if there's more than one, then the setup's
own `AGENTS.md`.

## Keys and permissions

Signing in is one access key, for browsers, the desktop app and phones, and changing it signs them all out. A phone can
also scan a QR code from Settings that holds the hub's address and a one-time code.

The hub keeps the keys: GitHub, the providers, DigitalOcean and the env vars. A machine holds only the Claude token,
its own hub token and the project's env vars, so those should be test keys with spending caps.

Antigravity signs in on each machine with Google through `agy`'s own login, whose code a card in the thread takes.
Signed in that way, only I prompt its threads: agents can't send to or start one, and the hub doesn't continue it. A
Gemini API key in Settings lifts that. That's because Google allows a Google AI Pro or Ultra login in another app's ACP
client only while a person drives the session: not when another agent or a background queue writes the prompts, which
can get the account suspended (a Google maintainer's answer in google-antigravity/antigravity-cli issue #1493, November
2026). Google's guidance on this is still changing, so this may be revisited; `Hub::hands_off` is the one place in the
hub that applies it.

A machine's token lets it connect as a runner, download the runner and its setup kit (`shared/` and its own projects'
setups only), read and write the session files of its own threads and its project's, and use git through the hub for its
project's repos. It can't reach another project's threads, repos or setups.

A manager project is trusted with everything. Its machine gets the hub's own keys as the usual variables
(`GH_TOKEN`, `MODAL_TOKEN_ID` and `MODAL_TOKEN_SECRET`, `DIGITALOCEAN_ACCESS_TOKEN`, `AGENT37_API_KEY`,
`RAILWAY_API_TOKEN`, `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`, for the ones set), reaches any repo through the
hub with no push guard, merges without asking, and gets the tools that manage corra. Only the app can turn it on.

The hub sends a Content-Security-Policy with the app, and so does the built-in hub: scripts only from the app itself,
plus WebAssembly for the terminal; no frames; and connections only to its own address. The markdown sanitizer is no
longer the only guard against a script injected into a thread.

An agent can ask me for a secret with `secret.request`: a card in the thread takes it, and the hub passes it straight to
the thread's machine, which keeps it for a day in `~/.corra/secrets/`, readable only by the agent. The hub never stores
it, and it isn't in the thread's events.

Machines reach GitHub only through the hub. Their git remotes point at it, and it passes fetches and pushes on with its
own token, for the project's repos only. Machines push any branch and may delete any but the default one. A push to the
default branch or a tag waits for my yes in the machine's latest thread, like merging, unless the project allows them;
it waits four minutes, and after a later yes the same push (the same refs, from the same commits to the same commits)
goes through for an hour. A push that unpacks to more than 256 MB is refused. A merge shows me the pull request's title,
branches and head commit, and merges only that commit; if the branch has moved since, GitHub refuses and the agent is
told to ask again. Approvals wait in the hub's memory; when it restarts, any still open are cancelled with a notice in
their thread. An answer goes only to a request that still waits, and only once, so a second device or a double click is
refused. Pull requests, checks, issues and merging are hub tools. A workflow that runs on any branch still runs that
branch's code with the repo's secrets, so secrets belong in workflows that run on the default branch or tags.

What an agent may do is its own permission mode, Claude Code's or Antigravity's, picked per thread, plus for Claude Code
the rules in the kit's `claude-settings.json`. Corra adds no approvals of its own: every request the agent makes comes
to me.

A thread's keys, env vars and tools are fixed when its Claude Code process starts. When they've changed, the next
message to an idle thread restarts the process on the same session; a thread that's mid-turn keeps the old ones until
its next message.

Beside Claude Code's own tools, agents get corra's: other threads, GitHub, and in a manager project, managing corra
itself. A project can also turn on a browser on its machine and the Android test phone. The phone runs on an ordinary
machine of a kind with KVM, picked in Settings under Tools. On DigitalOcean its first start sets it up and keeps a
snapshot as its template, and each start after makes a new droplet from that. On Agent37 the same VM is stopped and
started. Either way the emulator starts from the same saved state and keeps nothing, so a phone's apps and data go when
it stops. Changing where it runs, or moving a project to another provider or kind, deletes what's left on the old one,
and lists in Settings anything that couldn't be deleted.

## The storage lease

The lease is `lease.json` in the bucket, and it's only ever written conditionally (`If-Match` on its ETag, or
`If-None-Match` when there is none), so two hubs can't both take it. A starting hub waits until the lease is released
or 30 seconds old, takes it, and reads it back. The running hub renews it every 5 seconds, checking first that it still
holds it. If another hub holds it, or it hasn't renewed for 15 seconds, it exits rather than keep writing. On SIGTERM
it stops renewing, has Litestream copy its last changes, then marks the lease released.

## Threads

A thread is a Claude Code session. The hub keeps its events and session files, so it survives a reset. Nothing is
deleted unless Settings says to delete hidden threads after some number of days: then, once a day, a thread hidden that
long goes with its events, session files, saved work and settings, unless a fork that hasn't started yet still needs it.
A thread can move between machines with its uncommitted work, fork from any turn, or rewind to before one, which puts
back the files it changed since. Any Claude Code session on a machine can come in as a thread, with its history; the
next message resumes the same session.

The model, effort and mode pickers show each agent's own ids and names: Claude Code's models and efforts from its
`initialize` reply, Antigravity's from its session. Claude Code reports only its current mode, so corra lists its mode
ids with Claude Code's own titles. The new-thread page uses the last list it saw for each agent.

A message sent while the agent is mid-turn goes one of two ways. Sent now, it reaches Claude Code at once, which reads
it at its next step. Queued, the hub holds it and sends it when the turn completes, one queued message per turn, in
order; a stopped or failed turn leaves the queue waiting. The hub keeps the queue, so it survives a restart and shows on
every device.

A background agent that keeps working after a turn ends doesn't open a new turn: its steps join the turn that started it
and the thread stays idle. Each subagent shows as a row that opens what it was asked, its steps and its result.
Background agents and commands still running are listed at the end of the thread, each with a Stop. A message sent
mid-turn is placed in the thread where it was sent. The hub remembers Claude Code's latest usage report and sends it to
every app. Drafts and read marks are kept per device, in the browser. A thread opens on its last four turns; earlier
ones load as you scroll up.

A command or agent that finishes in the background can wake Claude Code, which answers in a turn of its own that says
what woke it.

A turn that hits a usage limit fails once, with the time the limit resets, and the thread shows as limited. A minute
after the reset the hub sends it "Your usage limit has reset. Continue where you left off.", unless it's in the trash,
waits on me, or has started another turn since; the queue then drains as usual. After two of those in a row hit the
limit again, it stops.

Claude's prompt cache lasts about an hour on a subscription and five minutes with an API key, and a thread used after
that reads its whole history again at full price. Settings pick what happens to threads over a size: nothing; a Compact
button by Send once the cache has expired, which runs `/compact` before the message; or `/compact` sent by the hub to an
idle thread about ten minutes before its cache expires, falling back to the button when that's missed. A follower isn't
told about a `/compact` the hub sent.

A turn whose machine restarts, sleeps or is destroyed is closed as stopped, and its waiting requests are cancelled. Each
message carries an id from the app, so one sent twice goes once.

A thread can follow another: when the followed thread finishes a turn, stops with an error or waits on me, the hub
sends a short message into the follower, which wakes it. A thread started with `threads.start` is followed by the
thread that started it, and shows under it in the sidebar. A thread has one follower, following can't go in a circle,
and a stop the follower asked for isn't reported back to it.

When a thread waits on me, finishes or fails, the hub notifies me, unless one of my apps is open and focused. It sends
Web Push to each browser where I turned it on in Settings, and the desktop app shows them itself. Pushes are signed with
the hub's own VAPID key, kept with the credentials, and encrypted (RFC 8291), so the push service can't read them. A
finish isn't sent while the queue still holds a message, after a `/compact` the hub sent, or for a thread another thread
follows. Settings pick which of the three to send, and signing a device out ends its notifications.

## Not built

- Codex.
- Worktrees per thread, commit buttons, a file editor, notifications outside the app, iOS.
- More than one person per hub, git hosts other than GitHub.
