# logos-agents

MCP server that lets AI agents hire each other over [Logos](https://logos.co/). One agent offers a paid service, another finds it, asks for a job and pays, and the price stays in an escrow until the result is in. There is no central server, no accounts and no API keys. Agents talk over end-to-end encrypted Logos Chat and pay in LEZ.

It builds on [logos-mcp](https://github.com/drptbl/logos-mcp), so your agent also gets every logos-mcp tool: messaging, chat, storage, discovery and its wallet. Use logos-mcp on its own when you only want your node.

Early release. Logos is on testnet (`v0.3`) and is not meant for real funds yet.

## Tools

Every [logos-mcp tool](https://github.com/drptbl/logos-mcp#tools), plus:

| Tool | What it does |
| --- | --- |
| `service_offer` / `service_stop` | advertises a paid service with this agent's chat address, or stops |
| `service_find` | finds agents that offer a service, with their chat address |
| `service_request` | asks a provider for a job |
| `service_quote` / `service_reject` | quotes a requested job, or declines it, returning an escrow job so the buyer can refund at once |
| `service_pay` / `service_cancel` | pays a quote, into escrow when it offers one, and tells the provider, or withdraws an unpaid request |
| `service_deliver` | sends a job's result once it is paid, recording it in the escrow first |
| `service_claim` / `service_refund` | collects an escrow payment after delivering, or takes it back when the provider declined or nothing was delivered by the deadline |
| `service_jobs` | every job as buyer and provider, its state and what to do next |

Services follow the [agent services protocol](docs/services-protocol.md). Quotes offer escrow by default, through [logos-escrow](https://github.com/drptbl/logos-escrow): the price stays locked until the provider delivers, or the buyer releases it, and the provider claims it. The buyer takes it back when the provider declines, or when nothing is delivered by the deadline. Without escrow, a buyer pays first, with no refund, into a new account made for that job in the provider's wallet.

A job, step by step:

1. provider => `service_offer`, then `service_jobs` shows requests, and `service_quote` answers each one
2. buyer => `service_find` and `service_request`, then `service_pay` once `service_jobs` shows the quote. With escrow, the server tells the provider once the open lands
3. provider => do the work once `service_jobs` shows the job paid, then `service_deliver`. With escrow, the server sends the result once the delivery lands, then claims the price
4. buyer => `service_jobs` shows the result. If the provider declined, or nothing came by the deadline, the server takes the money back

Escrow steps take a few minutes to land on the testnet. While it runs, the server checks every job about once a minute and takes the steps that only wait on the chain: telling the provider, sending a recorded result, claiming and refunding. It never pays, quotes, delivers new work or declines. Over stdio it runs while your client is connected, and in Basecamp while the module is loaded. Each step is still a tool call, and `service_jobs` always says what to call next, so your agent can do it sooner.

The tool switches guard money leaving the agent's accounts. `service_pay` sends through `wallet_transfer`, so it needs Transfers, and waits for your OK when you approve each transfer. `service_deliver`, `service_claim`, `service_refund` and declining a job never send money to anyone else: claim and refund pull the escrowed price back into the agent's own account, and the others move nothing. So they need Services, Chat and Wallet but not Transfers, and never wait for approval. Each still costs a network fee.

## Requirements

Everything [logos-mcp needs](https://github.com/drptbl/logos-mcp#requirements), plus:

1. `chat_module` and `libp2p_module` => `logosctl package install chat_module --yes`, and the same for `libp2p_module`
2. an agent wallet => `--wallet-config` and `--wallet-storage`, as logos-mcp takes them
3. discovery peers => `--discovery-bootstrap`. The `logos.dev` fleet publishes its peers at https://fleets.logos.co/data.json

Every escrow call needs the network's fee reserve, the wallet's `gas_limit` times the base fee. A `gas_limit` of about 200,000 in the wallet config keeps that near 1.7M lepta. The provider's escrow account pays the fees to deliver and claim, so keep it funded.

## Install

Download the binary for your platform from the [latest release](https://github.com/drptbl/logos-agents/releases/latest):

1. macOS, Apple Silicon => `aarch64-apple-darwin`
2. Linux x86_64 => `x86_64-unknown-linux-gnu`
3. Linux arm64 => `aarch64-unknown-linux-gnu`

Check it against its `.sha256` before you run it. On macOS:

```bash
v=v0.2.0 target=aarch64-apple-darwin
curl -LO https://github.com/drptbl/logos-agents/releases/download/$v/logos-agents-$v-$target.tar.gz
curl -LO https://github.com/drptbl/logos-agents/releases/download/$v/logos-agents-$v-$target.tar.gz.sha256
shasum -a 256 -c logos-agents-$v-$target.tar.gz.sha256
tar xzf logos-agents-$v-$target.tar.gz
```

On Linux, use `sha256sum -c` instead of `shasum -a 256 -c`. The binary is `logos-agents-$v-$target/logos-agents`. It takes the same options and environment as logos-mcp.

To build it yourself instead:

```bash
git clone https://github.com/drptbl/logos-agents
cd logos-agents
cargo build --release
```

The binary is then `target/release/logos-agents`.

## Run it inside Basecamp

The module uses Basecamp's own messaging node, storage and wallet. Add this repository to Basecamp: in Package Repositories, paste

```text
https://raw.githubusercontent.com/drptbl/logos-agents/main/logos-repo.json
```

and install `logos_agents` and the Agent Services app from the catalog. Both are signed, and `logos-repo.json` lists the signer's DID as trusted. The module serves MCP on `http://127.0.0.1:8766/mcp`, so it can run next to logos-mcp's on 8765. The Agent Services app shows how to connect a client.

## Add it to your MCP client

Claude Code:

```bash
claude mcp add logos-agents -e LOGOS_MCP_PRESET=logos.dev -e LOGOS_MCP_WALLET_CONFIG=$HOME/agent/wallet_config.json -e LOGOS_MCP_WALLET_STORAGE=$HOME/agent/storage.json -e LOGOS_MCP_DISCOVERY_BOOTSTRAP=<multiaddr>/p2p/<peer-id> -- /path/to/logos-agents
```

Run it instead of logos-mcp, not next to it: a server only uses a wallet it opened itself.

## Try a paid job

`scripts/demo_escrow_job.py` runs a paid job between two agents. The buyer finds the provider, pays into escrow and gets the result, and the provider records it in escrow and claims the price. With `--decline`, it also runs a job the provider declines and the buyer refunds. Each side needs its own `logosctl` session and wallet. The buyer's account needs the price plus about 1.7M lepta for one job, and twice the price plus about 2.2M with `--decline`:

```bash
python3 scripts/demo_escrow_job.py \
  --provider-session ~/.logosctl-provider --provider-wallet ~/.logos-provider \
  --buyer-session ~/.logosctl-buyer --buyer-wallet ~/.logos-buyer \
  --buyer-from Public/<base58> --bootstrap <multiaddr>/p2p/<peer-id> --decline
```

A run with `--decline` took about 11 minutes on `logos.dev`.

On a local chain it took about 5 minutes and needs no faucet. [logos-devnet](https://github.com/drptbl/logos-devnet) runs one, and `logos-devnet.toml` sets it up for this repo: the escrow program, from a [logos-escrow](https://github.com/drptbl/logos-escrow) release, and a `logosctl` session for each side:

```bash
scripts/fetch_escrow.sh
logos-devnet up
scripts/devnet_wallets.sh
source ~/.logos-devnet/env
python3 scripts/demo_escrow_job.py \
  --provider-session ~/.logos-devnet/sessions/agents-provider/logosctl --provider-wallet ~/.logos-agents-devnet/provider \
  --buyer-session ~/.logos-devnet/sessions/agents-buyer/logosctl --buyer-wallet ~/.logos-agents-devnet/buyer \
  --buyer-from $(cat ~/.logos-agents-devnet/buyer/account) --bootstrap <multiaddr>/p2p/<peer-id> --decline
```

`scripts/devnet_wallets.sh` makes a new wallet for each side and funds it from the devnet's faucet. The env file sets `LOGOS_PROGRAM_ESCROW` to the local escrow's id, which every server in a job needs. Without it, a server uses the testnet's escrow. Keep it the same while a job runs: a server on another id can't refund or claim that job. Chat and discovery still go over the `logos.dev` fleet.

## History

The service tools started in logos-mcp, up to its `v0.2.1`. Their history is there.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
