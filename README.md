<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="app/assets/bundled/svg/smash-logo-light.svg">
    <img src="app/assets/bundled/svg/smash-logo-dark.svg" width="150" alt="Smash logo">
  </picture>
</p>

<h1 align="center">Smash</h1>

<p align="center">
  A local-first agentic terminal for your computer and every machine you SSH into.
</p>

Smash combines a modern terminal, project navigation, code review, and a native AI agent in one
desktop application. It connects directly to your ChatGPT subscription or to models served by
Ollama and LM Studio. It does not require a Warp account, Warp AI subscription, Codex process, or
separate proxy application.

Smash is built from the open-source [Warp terminal](https://github.com/warpdotdev/warp) client.

## Why Smash exists

Most coding agents work well in a local repository, but become awkward when the work happens on a
server. You either install another agent and its credentials on every remote machine, or give up the
terminal UI and workflows available on your own computer.

Smash keeps the agent on your computer and lets it work through the terminal session you already
control. SSH into a machine, ask Smash to inspect or fix something, review its commands and diffs,
and keep the model configuration and credentials local.

## What it provides

- Native agent mode integrated into the terminal—not a wrapped CLI agent.
- Direct ChatGPT subscription connection through browser OAuth, with tokens stored in the system
  keychain.
- Local model discovery and inference through configurable Ollama and LM Studio server URLs.
- A model picker that shows only models available from your connected providers.
- SSH-aware agent actions: commands run through the active remote shell without installing Smash,
  Codex, Claude, or another agent on the server.
- Session-based workspace navigation: ordered sessions in a configurable sidebar, with each
  session's ordered tabs kept across the top and restored from the local database.
- Project sidebar and file explorer for navigating local workspaces.
- Integrated editor, code-review panel, and diff viewer for inspecting agent changes.
- Terminal blocks, tabs, panes, command history, and the other core terminal workflows inherited
  from the Warp open-source client.
- A separate application identity, icon, data namespace, settings experience, and provider layer.

## How remote work flows

```text
Your Mac                     Selected model provider
┌───────────────────┐         ┌──────────────────────┐
│ Smash UI + agent  │<──────>│ ChatGPT / Ollama /   │
│ credentials local │         │ LM Studio            │
└─────────┬─────────┘         └──────────────────────┘
          │ active SSH terminal
          ▼
┌───────────────────┐
│ Remote server     │
│ shell + your tools│
└───────────────────┘
```

The remote server only needs SSH and the tools required for the task. Local providers such as
Ollama and LM Studio run separately; their model files are not copied into the Smash application.

## Model providers

Open **Settings → AI Providers** to configure providers.

### ChatGPT subscription

Select **Connect** next to **ChatGPT subscription** and complete browser authentication. Smash owns
the OAuth flow, refreshes the session, streams responses, and executes tool calls directly. It does
not launch Codex or depend on another application running in the background.

### Ollama

Start Ollama and set its server URL in Smash. The default is:

```text
http://127.0.0.1:11434
```

### LM Studio

Start LM Studio's local API server and set its URL in Smash. The default is:

```text
http://127.0.0.1:1234
```

Discovered models from connected providers appear in the model picker and the `/model` command.

## Build and run

Smash is currently developed and tested on macOS.

```sh
./script/bootstrap
WARP_SKIP_COMMON_SKILLS_INSTALL=1 ./script/run --dont-open
open target/debug/bundle/osx/Smash.app
```

The internal Rust binary target remains `warp-oss` to reduce divergence from upstream. The built
application, bundle identifier, UI, and data paths identify as Smash.

## Project status

Smash is under active development. Expect rough edges while upstream account-backed features are
removed or replaced with local-first equivalents.

## Licensing and attribution

The client is distributed under the [GNU Affero General Public License v3](LICENSE-AGPL). The
separately licensed Warp UI crates remain under the [MIT License](LICENSE-MIT). Retain both license
files and applicable upstream attribution when redistributing modified builds.
