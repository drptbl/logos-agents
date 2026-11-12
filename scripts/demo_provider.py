#!/usr/bin/env python3
"""A provider that runs on its own, on the local devnet logos-devnet.toml sets up.

`logos-agents provide` serves one offer whose handler is a small HTTP server
started here, with no agent on the provider's side. The same server prices
each request by its word count, as the offer's quote handler. A buyer agent
finds it, asks for three jobs and pays for each into escrow:

1. the handler answers the first, and the buyer gets that output, then the
   provider's server claims the price
2. the handler declines the second, and the buyer's server takes the money back
3. the third's input is a file the buyer put in Logos Storage. The provider's
   server fetches it for the handler, uploads the handler's file in return, and
   the buyer downloads that by its CID

Run `logos-devnet up` in this repo first. logos-devnet and logosctl must be on
PATH, and target/debug/logos-agents built. Exits non-zero if a job does not
complete.
"""
import argparse
import base64
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import demo_escrow_job as demo  # noqa: E402

DECLINE = "please decline this one"
# What the quote handler charges per word of input.
PER_WORD = 50_000


def price_of(text):
    return str(max(1, len(text.split())) * PER_WORD)


def serve_handler():
    """A handler on this machine: it summarises a text, declines DECLINE, turns
    a file's content to upper case, and on /quote prices a request by its word
    count."""
    asked = []

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            job = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            text = job["input"].get("text", "")
            if self.path == "/quote":
                body = json.dumps({"price_lepta": price_of(text)}).encode()
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            asked.append(job)
            if "content_b64" in job["input"]:
                status = 200
                content = base64.b64decode(job["input"]["content_b64"]).upper()
                body = json.dumps({"output": {"content_b64": base64.b64encode(content).decode()}}).encode()
            elif text == DECLINE:
                status, body = 422, b"this handler won't do that one"
            else:
                status = 200
                body = json.dumps({"output": {"text": f"{len(text.split())} words, starting {text.split()[0]!r}"}}).encode()
            self.send_response(status)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_):
            pass

    server = HTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return f"http://127.0.0.1:{server.server_port}", asked


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--bin", default="target/debug/logos-agents")
    ap.add_argument("--price", default="1000000",
                    help="the offer's own price in lepta, which the quote handler replaces with its per-word price")
    ap.add_argument("--service", default=f"logos-agents-demo/provider/v{int(time.time())}")
    ap.add_argument("--timeout", type=int, default=600, help="seconds to wait for each step")
    ap.add_argument("--poll", type=demo.poll_secs, default=3, help="seconds between checks")
    args = ap.parse_args()
    demo.POLL = args.poll

    # Sessions, wallets, the escrow id and the messaging hub, from the devnet.
    sides = argparse.Namespace(provider_session=None, provider_wallet=None, buyer_session=None,
                               buyer_wallet=None, buyer_from=None, bootstrap=None)
    demo.on_devnet(sides)

    base, asked = serve_handler()
    url = f"{base}/run"
    logs = tempfile.mkdtemp(prefix="logos-mcp-demo-")
    # Each side's files root, for what goes through storage.
    provider_files = tempfile.mkdtemp(prefix="logos-agents-provider-files-")
    buyer_files = tempfile.mkdtemp(prefix="logos-agents-buyer-files-")
    offers = os.path.join(logs, "offers.toml")
    with open(offers, "w") as f:
        f.write(f'[[offer]]\nservice_id = "{args.service}"\ntag = "summarize"\n'
                f'price_lepta = "{args.price}"\ndeliver_within_s = 3600\nhandler = "{url}"\n'
                f'quote_handler = "{base}/quote"\nmax_jobs = 2\n')

    wallet = sides.provider_wallet
    provider_env = dict(
        os.environ,
        LOGOSCTL_CONFIG_DIR=sides.provider_session,
        LOGOS_MCP_PRESET="logos.dev",
        LOGOS_MCP_WALLET_CONFIG=os.path.join(wallet, "wallet_config.json"),
        LOGOS_MCP_WALLET_STORAGE=os.path.join(wallet, "storage.json"),
        LOGOS_MCP_DISCOVERY_BOOTSTRAP=sides.bootstrap,
        LOGOS_AGENTS_FOLLOW_THROUGH_SECS=str(args.poll),
        LOGOS_MCP_FILES_ROOT=provider_files,
    )
    provider = subprocess.Popen([args.bin, "provide", offers], env=provider_env, stdin=subprocess.DEVNULL,
                                stdout=subprocess.DEVNULL,
                                stderr=open(os.path.join(logs, "provider.log"), "w"))
    demo.log(f"[provider] logos-agents provide, handler {url}")
    buyer = demo.start("buyer", args.bin, sides.buyer_session, sides.buyer_wallet, sides.bootstrap,
                       "logos.dev", logs, files_root=buyer_files)
    try:
        def find():
            _, body = buyer.tool("service_find", service_id=args.service)
            return next((p for p in body.get("providers", []) if not p["this_node"]), None)
        found = demo.wait_for(find, "the provider's offer", 300, 5)
        demo.log(f"[buyer] found provider {found['peer_id']}")

        def job(text="", **input_):
            input_ = input_ or {"input_text": text}
            job = demo.call(buyer, "buyer", "service_request", chat_address=found["chat_address"],
                            service_id=args.service, **input_)["job"]
            demo.wait_for(lambda: demo.state(buyer, job) == "quoted", "the provider's quote", args.timeout)
            price = demo.job_in(buyer, job)["quote"]["price_lepta"]
            # The quote handler gets a CID without its content, so no words.
            if price != price_of(text):
                sys.exit(f"quoted {price} lepta, not the quote handler's {price_of(text)}")
            demo.log(f"[provider] quoted {price} lepta on its own, by the word")
            demo.call(buyer, "buyer", "service_pay", job=job, **{"from": sides.buyer_from})
            return job

        done = job(demo.DOCUMENT)
        result = demo.wait_for(
            lambda: (demo.job_in(buyer, done) or {}).get("output") if demo.state(buyer, done) == "delivered" else None,
            "the handler's output", args.timeout)
        demo.log(f"[buyer] result: {result['text']}")
        demo.wait_for(lambda: demo.escrow(buyer, done) == "claimed", "the provider's claim", args.timeout)
        demo.log("[provider] claimed the price on its own")

        declined = job(DECLINE)
        demo.wait_for(lambda: demo.escrow(buyer, declined) == "refunded", "the refund", args.timeout)
        demo.log("[buyer] the declined job's price is back")
        def transfer(session):
            _, t = buyer.tool("storage_transfer_status", session_id=session)
            if isinstance(t, dict) and t.get("done"):
                if not t.get("success"):
                    sys.exit(f"storage transfer {session} failed: {t}")
                return t
            return None

        with open(os.path.join(buyer_files, "input.txt"), "w") as f:
            f.write(demo.DOCUMENT)
        upload = demo.call(buyer, "buyer", "storage_upload", path="input.txt")["session_id"]
        cid = demo.wait_for(lambda: transfer(upload), "the input's upload", args.timeout)["cid"]
        demo.log(f"[buyer] put the input in storage as {cid}")
        stored = job(input_cid=cid)
        output = demo.wait_for(
            lambda: (demo.job_in(buyer, stored) or {}).get("output") if demo.state(buyer, stored) == "delivered" else None,
            "the handler's file", args.timeout)
        if "cid" not in output:
            sys.exit(f"the output came back as {output}, not a CID")
        demo.log(f"[buyer] the output is in storage as {output['cid']}")
        download = demo.call(buyer, "buyer", "storage_download", cid=output["cid"], path="output.txt")["session_id"]
        demo.wait_for(lambda: transfer(download), "the output's download", args.timeout)
        with open(os.path.join(buyer_files, "output.txt")) as f:
            if f.read() != demo.DOCUMENT.upper():
                sys.exit("the downloaded output isn't the handler's")
        demo.log("[buyer] downloaded the handler's file")
        demo.wait_for(lambda: demo.escrow(buyer, stored) == "claimed", "the provider's claim", args.timeout)
        if len(asked) != 3:
            sys.exit(f"the handler was asked {len(asked)} times, not three")
        if left := os.listdir(os.path.join(provider_files, "logos-agents-jobs")):
            sys.exit(f"the provider left {left} in its files root")
    finally:
        provider.terminate()
        provider.wait(timeout=60)
        buyer.close()
    print(f"DEMO: PASS (logs in {logs})")


if __name__ == "__main__":
    main()
