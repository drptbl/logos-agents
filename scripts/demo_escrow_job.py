#!/usr/bin/env python3
"""Paid agent-to-agent job over Logos, through escrow, driven through two
logos-mcp servers with the service_* tools.

provider: offers a service, quotes the job and records the result in escrow.
          Its server sends the result once that lands, then claims the price.
buyer:    finds the provider, asks for the job and pays into escrow. Its
          server tells the provider once the open lands. Then it reads the
          result.

Each tool is called once: the servers take the steps that only wait on the
chain. With --decline, a second job is declined instead: the provider returns
it in escrow and the buyer's server takes the money back.

Each side needs its own logosctl session and wallet (lez_core holds one wallet
per daemon), with lez_core, chat_module and libp2p_module installed. Every
escrow call needs the network's fee reserve, gas_limit x base fee, about 1.7M
lepta at a gas_limit of 200,000. So the buyer needs about the price plus 1.7M
lepta for one job, and twice the price plus 2.2M with --decline; the provider
needs about 1.7M. Each escrow step takes a few minutes to land on the testnet. libp2p_module 1.1.0 lets a
node be found only for the first service it advertises, so reload it on the
provider's session between runs: logosctl module reload libp2p_module. A
server only uses a wallet it opened itself, so if another logos-mcp server
used a session since lez_core loaded, reload lez_core there too.

With --devnet, it runs on the local devnet that `logos-devnet up` starts from
logos-devnet.toml instead: the sessions, a funded wallet per side and the escrow
id come from it, and only logos-devnet and logosctl need to be on PATH.

Exits non-zero if a job does not complete.
"""
import argparse
import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.request

SCRIPTS = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, SCRIPTS)
from e2e import Client  # noqa: E402

# Seconds between checks, for the demo and both servers' follow-through. None
# keeps each wait's own pace and the servers' default of a minute.
POLL = None

DOCUMENT = ("Logos is a privacy-first stack for decentralised apps. It combines a blockchain with "
            "private execution, peer-to-peer messaging, content-addressed storage and a mixnet.")


def start(name, bin_, session, wallet_dir, bootstrap, preset, log_dir):
    env = dict(
        os.environ,
        LOGOSCTL_CONFIG_DIR=session,
        LOGOS_MCP_PRESET=preset,
        LOGOS_MCP_WALLET_CONFIG=os.path.join(wallet_dir, "wallet_config.json"),
        LOGOS_MCP_WALLET_STORAGE=os.path.join(wallet_dir, "storage.json"),
        LOGOS_MCP_DISCOVERY_BOOTSTRAP=bootstrap,
    )
    if POLL:
        env["LOGOS_AGENTS_FOLLOW_THROUGH_SECS"] = str(POLL)
    c = Client([bin_], env, open(os.path.join(log_dir, f"{name}.log"), "w"))
    c.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                             "clientInfo": {"name": name, "version": "0"}})
    c.notify("notifications/initialized")
    return c


def log(*parts):
    print(time.strftime("%H:%M:%S"), *parts, flush=True)


FLEETS = "https://fleets.logos.co/data.json"


def fleet_peers():
    """logos.dev's discovery peers from the fleet list, or from scripts/logos-dev-peers.txt
    when the list doesn't answer."""
    for _ in range(3):
        try:
            # The site refuses Python's default User-Agent.
            ask = urllib.request.Request(FLEETS, headers={"User-Agent": "logos-agents-demo"})
            with urllib.request.urlopen(ask, timeout=20) as r:
                nodes = json.load(r)["logos.dev"]["logos-node-delivery"].values()
            peers = [n["ServiceMeta"]["libp2p_multiaddr"] + "/p2p/" + n["ServiceMeta"]["libp2p_peer_id"]
                     for n in nodes if n["ServiceMeta"].get("libp2p_peer_id")]
            if peers:
                return ",".join(peers)
        except (OSError, ValueError, KeyError, TypeError) as e:
            log(f"the fleet list failed: {e}")
        time.sleep(5)
    log("using scripts/logos-dev-peers.txt instead")
    with open(os.path.join(SCRIPTS, "logos-dev-peers.txt")) as f:
        return ",".join(line.strip() for line in f if line.strip() and not line.startswith("#"))


def devnet(*args):
    """A logos-devnet command's JSON output."""
    out = subprocess.run(["logos-devnet", "--json", *args], capture_output=True, text=True, check=True)
    return json.loads(out.stdout)


def on_devnet(args):
    """Fills in sessions, wallets and the escrow id from the devnet that `logos-devnet up`
    started from logos-devnet.toml: both servers take its env, and each side gets a new
    wallet, funded from its faucet."""
    os.environ.update(devnet("env"))
    for side in ("provider", "buyer"):
        setattr(args, f"{side}_session", devnet("session", "env", f"agents-{side}")["LOGOSCTL_CONFIG_DIR"])
    wallets = tempfile.mkdtemp(prefix="logos-agents-devnet-wallets-")
    made = subprocess.run([os.path.join(SCRIPTS, "devnet_wallets.sh"), wallets],
                          capture_output=True, text=True, check=True)
    args.provider_wallet = os.path.join(wallets, "provider")
    args.buyer_wallet = os.path.join(wallets, "buyer")
    args.buyer_from = made.stdout.strip()
    log(f"devnet wallets in {wallets}, buyer {args.buyer_from}")


def call(c, who, tool, **args):
    ok, body = c.tool(tool, **args)
    if not ok:
        sys.exit(f"[{who}] {tool} failed: {body}")
    log(f"[{who}] {tool}")
    return body


def wait_for(fn, what, timeout, every=15):
    deadline = time.time() + timeout
    while time.time() < deadline:
        value = fn()
        if value:
            return value
        time.sleep(POLL or every)
    sys.exit(f"timed out waiting for {what}")


# The server holds back a second escrow call while the first may still land,
# and lets it go once that one counts as dropped, after 15 minutes.
RESEND_AFTER = 16 * 60


def until_landed(c, who, tool, args, landed, what, timeout):
    """Wait for an escrow call to land, calling the tool again now and then,
    as an agent would: a call the network dropped is then sent once more."""
    deadline = time.time() + timeout
    tried = time.time()
    while time.time() < deadline:
        if landed():
            return
        if time.time() - tried > RESEND_AFTER:
            ok, body = c.tool(tool, **args)
            log(f"[{who}] {tool} again: {'sent' if ok else body}")
            tried = time.time()
        time.sleep(POLL or 45)
    sys.exit(f"timed out waiting for {what}")


def job_in(c, job):
    _, body = c.tool("service_jobs")
    return next((j for j in body.get("jobs", []) if j["job"] == job), None)


def state(c, job):
    return (job_in(c, job) or {}).get("state")


def escrow(c, job):
    return ((job_in(c, job) or {}).get("escrow") or {}).get("state")


def paid_job(provider, buyer, chat_address, args, text):
    """Request, quote and pay a job; returns its id once the provider sees it paid."""
    job = call(buyer, "buyer", "service_request", chat_address=chat_address,
               service_id=args.service, input_text=text)["job"]
    wait_for(lambda: state(provider, job) == "requested", "the request", 300, 5)
    quote = call(provider, "provider", "service_quote", job=job)["quote"]
    if not quote.get("escrow"):
        sys.exit("the quote offers no escrow")
    wait_for(lambda: state(buyer, job) == "quoted", "the quote", 300, 5)
    opened = call(buyer, "buyer", "service_pay", job=job, **{"from": args.buyer_from})
    log(f"[buyer] escrow open {opened.get('escrow_open_tx')} sent, its server tells the provider once it lands")
    wait_for(lambda: state(provider, job) == "paid", "the provider to see it paid", args.timeout, 30)
    log(f"[provider] job {job} is paid in escrow")
    return job


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--bin", default="target/debug/logos-agents")
    ap.add_argument("--devnet", action="store_true",
                    help="run on the devnet logos-devnet.toml sets up: sessions, wallets and the escrow come from it")
    ap.add_argument("--provider-session")
    ap.add_argument("--provider-wallet", help="folder with wallet_config.json and storage.json")
    ap.add_argument("--provider-escrow", help="provider's Public/<base58> account escrow pays (default: its best-funded)")
    ap.add_argument("--buyer-session")
    ap.add_argument("--buyer-wallet")
    ap.add_argument("--buyer-from",
                    help="buyer's Public/<base58> account, holding the price plus the network's fee reserve")
    ap.add_argument("--bootstrap", help="comma-separated /p2p/ discovery multiaddrs (default: logos.dev's)")
    ap.add_argument("--preset", default="logos.dev")
    ap.add_argument("--price", default="1000000", help="lepta per job")
    ap.add_argument("--service", default=f"logos-mcp-demo/summarize/v{int(time.time())}")
    ap.add_argument("--decline", action="store_true", help="also run a job the provider declines")
    ap.add_argument("--timeout", type=int, default=1800, help="seconds to wait for each escrow step")
    ap.add_argument("--poll", type=int, help="seconds between checks, for the demo and both servers: "
                    "a few suit a local devnet, whose blocks land in a second")
    args = ap.parse_args()
    global POLL
    POLL = args.poll
    if args.devnet:
        on_devnet(args)
    missing = [f"--{a.replace('_', '-')}" for a in
               ("provider_session", "provider_wallet", "buyer_session", "buyer_wallet", "buyer_from")
               if not getattr(args, a)]
    if missing:
        ap.error(f"{', '.join(missing)} needed, or --devnet")
    args.bootstrap = args.bootstrap or fleet_peers()

    logs = tempfile.mkdtemp(prefix="logos-mcp-demo-")
    provider = start("provider", args.bin, args.provider_session, args.provider_wallet, args.bootstrap, args.preset, logs)
    buyer = start("buyer", args.bin, args.buyer_session, args.buyer_wallet, args.bootstrap, args.preset, logs)

    offer = {"service_id": args.service, "tag": "summarize", "price_lepta": args.price, "deliver_within_s": 3600}
    if args.provider_escrow:
        offer["escrow_account"] = args.provider_escrow
    call(provider, "provider", "service_offer", **offer)

    def find():
        _, body = buyer.tool("service_find", service_id=args.service)
        return next((p for p in body.get("providers", []) if not p["this_node"]), None)
    found = wait_for(find, "the provider's offer", 300, 10)
    log(f"[buyer] found provider {found['peer_id']}")

    # A delivered job: the result is recorded in escrow before it's sent,
    # then the provider claims the price.
    job = paid_job(provider, buyer, found["chat_address"], args, DOCUMENT)
    words = len(DOCUMENT.split())
    summary = f"{DOCUMENT.split('. ')[0].rstrip('.')}. ({words} words)"
    deliver = {"job": job, "output_text": summary}
    call(provider, "provider", "service_deliver", **deliver)
    until_landed(provider, "provider", "service_deliver", deliver,
                 lambda: escrow(provider, job) in ("delivered", "claimed"), "the delivery to land", args.timeout)
    result = wait_for(lambda: (job_in(buyer, job) or {}).get("output") if state(buyer, job) == "delivered" else None,
                      "the provider's server to send the result", 600)
    log(f"[buyer] result: {result['text']}")
    wait_for(lambda: escrow(provider, job) == "claimed", "the provider's server to claim the price", args.timeout, 45)
    log("[provider] the price is claimed")

    # A declined job: Return moves nothing, then the buyer refunds at once.
    if args.decline:
        job = paid_job(provider, buyer, found["chat_address"], args, "a job to decline")
        call(provider, "provider", "service_reject", job=job, reason="can't do it after all")
        wait_for(lambda: escrow(buyer, job) == "refunded", "the buyer's server to take the money back", args.timeout, 45)
        log("[buyer] the price is back")

    call(provider, "provider", "service_stop", service_id=args.service)
    provider.close()
    buyer.close()
    print(f"DEMO: PASS (logs in {logs})")


if __name__ == "__main__":
    main()
