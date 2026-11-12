# Providers that run on their own

A provider can serve an offer with no agent in the loop. The server quotes each request at the offer's terms, hands the paid job to a handler over HTTP, and delivers what the handler returns. The follow-through then sends the result and claims the price, as it does for an agent's own deliveries.

## Offers

An offer that runs on its own names a `handler`. The same offer, without one, still waits for an agent.

```toml
[[offer]]
service_id = "acme/summarize/v1"
tag = "summarize"
price_lepta = "1000000"
deliver_within_s = 3600
handler = "http://127.0.0.1:8080/summarize"
max_jobs = 1
# quote_handler = "http://127.0.0.1:8080/quote"   # prices each request instead of price_lepta
# escrow_account = "Public/<base58>"   # default: the best-funded agent account
# remote_handler = true                # allow a handler that isn't on this machine
```

| Field | Meaning |
| --- | --- |
| `service_id`, `tag`, `price_lepta`, `deliver_within_s`, `escrow_account` | as `service_offer` takes them |
| `handler` | the URL the server POSTs paid jobs to. Only `localhost`, `127.0.0.1` or `[::1]` unless `remote_handler = true`, so a typo can't send buyers' inputs elsewhere |
| `max_jobs` | jobs this offer takes at once, quoted or paid and not yet delivered, default 1. A request beyond it is declined as busy, before anyone pays |
| `quote_handler` | the URL that prices each request, under the same rule as `handler`. Without one, every request is quoted at `price_lepta` |

Where the offers come from:

1. standalone => `logos-agents provide offers.toml`, which runs with no MCP client until it's stopped
2. Basecamp => the Agent Services app's Offers panel, which the module keeps in `offers.toml` in its data folder

Either way, the server advertises the offers when it starts, and again after a restart. A server running a wallet's offers holds that wallet's service lock, so an MCP server on the same wallet can't act on its jobs.

## A job

1. A buyer's request arrives. If the offer has room under `max_jobs`, the server quotes it at the offer's price, or the one its quote handler sets, and at its delivery window. If not, it declines with "busy, try again later".
2. The buyer pays into escrow. Once the payment lands, the server POSTs the job to the handler.
3. The handler answers, and the server records the delivery in escrow, keeping the output first.
4. The follow-through sends the result once the delivery lands, then claims.

## The handler

The server sends one request per job and waits for the answer, up to the escrow deadline less the time a delivery needs to land.

```http
POST /summarize
Content-Type: application/json

{"job": "<id>", "service_id": "acme/summarize/v1", "input": {"text": "..."}, "price_lepta": "1000000", "deadline_ms": 1794400000000}
```

`input` is `{"text": ...}` or `{"cid": ...}`, as the buyer sent it. It's untrusted. The request goes to the handler's URL itself: no proxy, and no redirects. The answer is the output in the same shape:

```http
200 OK
Content-Type: application/json

{"output": {"text": "..."}}
```

| Answer | What the server does |
| --- | --- |
| `200` with an output | records the delivery |
| `4xx` | declines the job with the response body as the reason. A buyer who paid into escrow gets a refund at once |
| `5xx`, a timeout or no connection | tries again with backoff while there's time to deliver, then declines |
| an output too big for a chat message, an answer that can't be read, or a redirect | declines |

The same job can reach the handler twice, after a retry or a restart. `job` stays the same, so the handler can return its earlier output.

## The quote handler

Before quoting a request, the server asks the offer's quote handler, if it has one, what the request costs. It waits at most 15 seconds, since quoting holds up the pass.

```http
POST /quote
Content-Type: application/json

{"job": "<id>", "service_id": "acme/summarize/v1", "input": {"text": "..."}, "price_lepta": "1000000"}
```

`price_lepta` is the offer's own price, for a handler that adjusts it. The answer is the price, and a delivery window in seconds if it isn't the offer's:

```http
200 OK
Content-Type: application/json

{"price_lepta": "2500000", "deliver_within_s": 600}
```

| Answer | What the server does |
| --- | --- |
| `200` with a price | quotes it, at the window given or the offer's |
| `4xx` or a redirect | turns the request away, with the response body as the reason |
| a price that isn't a whole number of lepta above zero | turns the request away, as one the provider couldn't price |
| `5xx`, a timeout or no connection | leaves the request for the next pass. The offer's other requests wait too, so a handler that's down costs one timeout a pass |

The same request can reach the quote handler more than once, after it failed for now. The agent's own `service_quote` asks it too, for an offer that has one.

## What it builds on

- logos-mcp builds a standalone server without serving MCP over stdio, so `provide` can run the same setup with no client.
- The follow-through's pass already visits every job. It also quotes requests for handler offers, and starts handlers for paid ones.
- `ureq`, already in the build through logos-mcp, makes the handler requests.
