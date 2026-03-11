---
name: limitless-btc-order
version: 0.1.0
description: "Read the YES/NO signal from limitless-btc-signal, fetch available USDC balance from Limitless Exchange, calculate order size (max 10% of available funds), and place the order using limitless-cli. Requires LIMITLESS_API_KEY, LIMITLESS_PRIVATE_KEY, and limitless-cli on PATH."
activation:
  keywords:
    - place order limitless
    - limitless order
    - execute signal limitless
    - buy limitless
    - bet limitless
    - submit order limitless
    - execute limitless
    - place bet limitless
    - limitless trade
    - execute trade limitless
  patterns:
    - "place.*order.*limitless"
    - "execute.*signal.*limitless"
    - "limitless.*(order|trade|buy|bet|execute)"
    - "(buy|sell|bet).*(yes|no).*limitless"
  tags:
    - trading
    - btc
    - limitless
    - order
    - execution
  max_context_tokens: 2000
metadata:
  openclaw:
    requires:
      bins:
        - limitless-cli
      env:
        - LIMITLESS_API_KEY
        - LIMITLESS_PRIVATE_KEY
---

# Limitless BTC 15m — Order Execution

Use this skill to execute a YES/NO order on Limitless Exchange based on the signal stored by `limitless-btc-signal`. Order size is capped at **10% of available USDC funds**.

**Prerequisites:**
- `binance-btc-ta` has run and written fresh TA data (< 30 min)
- `limitless-btc-signal` has run and written a signal to memory
- `limitless-cli` is installed on PATH
- `LIMITLESS_API_KEY` and `LIMITLESS_PRIVATE_KEY` environment variables are set

**Why `limitless-cli`:** Limitless Exchange orders require EIP-712 wallet signatures (on-chain Base network). The `http` tool cannot sign transactions — `limitless-cli` handles signing internally using `LIMITLESS_PRIVATE_KEY`.

---

## Mandatory Execution Flow

Run every step in order. Abort if any guard condition fails.

---

### Step 1 — Load the signal from memory

```
memory_read("limitless/btc-15m/signal")
```

**Abort conditions (do NOT place any order if):**
- Memory key is missing → ask user to run `limitless-btc-signal` first
- `fetched_at` is older than 10 minutes → signal is stale; ask user to refresh
- `direction` is missing or `score` < 5 → no actionable signal
- All markets in `markets[]` have `recommendation: "SKIP"` → nothing to trade

Extract from the signal:
- `direction` (LONG or SHORT)
- `score` (integer 0–10)
- `markets[]` — list of markets with `recommendation` (YES/NO/SKIP), `marketId`, `slug`, `yes_price`, `no_price`, `liquidity`

Filter `markets[]` to only those where `recommendation` is `"YES"` or `"NO"`. If none remain — abort.

---

### Step 2 — Fetch available USDC balance

Use the `http` tool with the `LIMITLESS_API_KEY` value from the environment (loaded from `.env` at startup). Pass it as the `X-API-Key` request header:

```
http tool:
  method: GET
  url: https://api.limitless.exchange/portfolio/trading/allowance?type=clob
  headers:
    X-API-Key: {value of LIMITLESS_API_KEY env var}
```

Parse the response to extract `availableBalance` (USDC, decimal). This is the USDC balance approved and available for CLOB trading.

If the response does not contain a numeric balance field, try:

```
http tool:
  method: GET
  url: https://api.limitless.exchange/portfolio
  headers:
    X-API-Key: {LIMITLESS_API_KEY}
```

Look for fields: `usdcBalance`, `balance`, `availableBalance`, or `collateral`. Use the first numeric field found.

**Abort if balance is 0 or cannot be parsed.** Report the raw response so the user can inspect it.

---

### Step 3 — Calculate order size

```
order_size_usdc = floor(available_balance * 0.10 * 100) / 100   # round down to 2 decimal places
```

**Guards:**
- If `order_size_usdc < 1.00` → abort: "Insufficient funds (< $1.00 per order). Available: ${available_balance}"
- If `order_size_usdc > available_balance` → clamp to `available_balance * 0.10`
- Check per-market liquidity: skip any market where `liquidity < order_size_usdc * 3` (thin book)

Report before placing orders:
```
Available balance: $XXX.XX USDC
Order size (10%): $XX.XX USDC
Markets to execute: N
```

For amounts ≤ $50 per order, proceed automatically. For amounts > $50, check for pre-approval in memory:

```
memory_read("limitless/order-approval")
```

If the key exists and is not empty → proceed. If missing → abort silently (log only). Do NOT ask the user in routine context.

To pre-approve large orders, ask IronClaw once: _"Approve Limitless orders above $50"_ — it will write the approval key.

---

### Step 4 — Place orders via `limitless-cli`

For each market in the filtered list (YES or NO recommendations only):

Determine the outcome flag:
- `recommendation: "YES"` → `--outcome yes`
- `recommendation: "NO"` → `--outcome no`

Determine the order type:
- Signal score 9–10 → FOK (Fill or Kill — market order, immediate execution)
- Signal score 5–8 → GTC (Good Till Cancelled — limit order at signal price)

**For GTC limit orders**, use the price from the signal:
- `recommendation: "YES"` → `--price {yes_price}` (e.g. 0.61)
- `recommendation: "NO"` → `--price {no_price}` (e.g. 0.39)

Execute the shell command:

```
shell tool:
  command: limitless-cli order \
    --slug "{market_slug}" \
    --side buy \
    --outcome {yes|no} \
    --price {price} \
    --size {order_size_usdc} \
    --order-type {GTC|FOK}
```

Environment variables `LIMITLESS_API_KEY` and `LIMITLESS_PRIVATE_KEY` must be present in the shell environment. `limitless-cli` reads them automatically.

**Example command (GTC, score 7):**
```
limitless-cli order \
  --slug "btc-above-97000-15m" \
  --side buy \
  --outcome yes \
  --price 0.61 \
  --size 24.50 \
  --order-type GTC
```

**Example command (FOK, score 9):**
```
limitless-cli order \
  --slug "btc-above-97000-15m" \
  --side buy \
  --outcome yes \
  --size 24.50 \
  --order-type FOK
```

Execute orders **one at a time**. Do not batch. Wait for each command to return before proceeding to the next.

---

### Step 5 — Handle order responses

For each order attempt, parse the `limitless-cli` output:

**Success indicators:** exit code 0, output contains `orderId`, `filled`, `status: "MATCHED"` or `"PENDING"`

**Failure indicators:** exit code non-zero, output contains `error`, `rejected`, `insufficient`

On failure:
- Log the error
- Do NOT retry automatically
- Continue to the next market

---

### Step 6 — Store order results in memory

After all orders are attempted:

```
memory_write:
  key: "limitless/btc-15m/orders/latest"
  content: <JSON with all order attempts, sizes, outcomes, timestamps>
```

JSON structure:
```json
{
  "executed_at": "2026-03-11T14:05:00Z",
  "signal_score": 7,
  "direction": "LONG",
  "available_balance_usdc": 245.00,
  "order_size_usdc": 24.50,
  "orders": [
    {
      "slug": "btc-above-97000-15m",
      "outcome": "yes",
      "price": 0.61,
      "size_usdc": 24.50,
      "order_type": "GTC",
      "status": "PENDING",
      "order_id": "...",
      "error": null
    }
  ]
}
```

---

### Step 7 — Report summary

Output a compact execution summary:

```
## Limitless Order Execution — {timestamp} UTC

Signal: LONG | Score: 7/10
Available: $245.00 USDC | Order size: $24.50 (10%)

| Market | Outcome | Price | Size | Type | Status |
|--------|---------|-------|------|------|--------|
| btc-above-97000-15m | YES | 0.61 | $24.50 | GTC | PENDING |
| btc-above-96500-15m | YES | 0.74 | $24.50 | GTC | FILLED  |
| btc-above-97500-15m | NO  | 0.39 | $24.50 | GTC | ERROR: insufficient liquidity |

Results stored → limitless/btc-15m/orders/latest
```

---

## Execution Rules (non-negotiable)

- **Never exceed 10% of available balance per order.** Calculate fresh each run — never use a cached balance.
- **Never place an order if signal score < 5.** Abort and report clearly.
- **Never place an order on a stale signal (> 10 min old).** The 15m window may have already closed.
- **Never retry a failed order automatically.** Report the failure and let the user decide.
- **Never place a SELL order.** This skill only places BUY orders for YES or NO outcomes.
- **Gate on memory approval if total exposure > $50.** Check `limitless/order-approval` memory key. Abort silently if missing — never prompt the user in routine context.
- **Skip any market with liquidity < 3× order size.** Thin books cause excessive slippage.

---

## Scheduled Routine Setup

To run this skill automatically every 15 minutes at T+6 minutes (fires at :06, :21, :36, :51 of every hour — 2 minutes after `limitless-signal-15m` completes), create the following routine once:

> _"Set up the limitless-order-15m routine"_

IronClaw will call `routine_create` with these exact parameters:

```
routine_create:
  name: "limitless-order-15m"
  description: "Read YES/NO signal from memory, fetch live USDC balance, place orders on Limitless Exchange via limitless-cli. Max 10% per order."
  trigger_type: "cron"
  schedule: "0 6,21,36,51 * * * *"
  action_type: "full_job"
  cooldown_secs: 840
  tool_permissions:
    - http
    - shell
    - memory_read
    - memory_write
  prompt: |
    Execute Limitless BTC 15m orders based on the stored signal.
    1. Read the signal from memory:
       memory_read("limitless/btc-15m/signal")
       Abort if: missing | fetched_at older than 10 minutes | score < 5 | no YES/NO markets.
    2. Fetch live USDC balance:
       GET https://api.limitless.exchange/portfolio/trading/allowance?type=clob
       Header: X-API-Key: {LIMITLESS_API_KEY env var}
       Extract availableBalance. Abort if 0 or unparseable.
       If no numeric balance field found, try:
       GET https://api.limitless.exchange/portfolio
       Header: X-API-Key: {LIMITLESS_API_KEY env var}
    3. Calculate order size:
       order_size = floor(availableBalance * 0.10 * 100) / 100
       Abort if order_size < 1.00.
    4. Filter signal markets: keep only recommendation=YES or NO.
       Skip any market where liquidity < order_size * 3.
       Abort silently (log only) if none remain after filtering.
    5. Place each order via shell tool — one at a time, wait for result before next:
       For score 9–10 (FOK):
         limitless-cli order --slug "{slug}" --side buy --outcome {yes|no} --size {order_size} --order-type FOK
       For score 5–8 (GTC):
         limitless-cli order --slug "{slug}" --side buy --outcome {yes|no} --price {yes_price|no_price} --size {order_size} --order-type GTC
    6. Store results:
       memory_write("limitless/btc-15m/orders/latest", <JSON: executed_at, score, direction, balance, order_size, orders[]>)
    7. Do not output a report — this is a background routine. Only log errors or order failures.
    Note: LIMITLESS_API_KEY and LIMITLESS_PRIVATE_KEY are loaded from .env automatically.
          Never confirm with the user — this is fully autonomous execution.
          Never place orders if total exposure (N_orders × order_size) > $50 without prior user approval stored in memory at "limitless/order-approval".
```

**Cron field reference (6-field format):**

```
0   6,21,36,51  *  *  *  *
│   │           │  │  │  └── weekday (any)
│   │           │  │  └───── month (any)
│   │           │  └──────── day (any)
│   │           └─────────── hour (any)
│   └─────────────────────── minute (6, 21, 36, 51)
└─────────────────────────── second (0)
```

**Full automated timing chain:**

```
:00:15  btc-ta-15m            cron: 15 0,15,30,45 * * * *  → ta/btc/{5m,15m,1h,4h}       (~30s)
:02:00  limitless-markets-15m cron: 0 2,17,32,47 * * * *   → limitless/btc-15m/snapshot   (~20s)
:04:00  limitless-signal-15m  cron: 0 4,19,34,49 * * * *   → limitless/btc-15m/signal     (~15s)
:06:00  limitless-order-15m   cron: 0 6,21,36,51 * * * *   → orders placed                (~30s)
          ↑ fully autonomous — candle close to order in ~6 minutes
```

**`tool_permissions: [shell]`** — the `shell` tool requires explicit pre-authorization in routine context since it is an `Always`-approval tool. This grants it automatically when the routine fires.

To verify:
```
routine_list
```

To manually trigger:
```
routine_fire name="limitless-order-15m"
```

---

## Skill Chain

```
1. binance-btc-ta         → writes ta/btc/{5m,15m,1h,4h}
2. limitless-btc-markets  → writes limitless/btc-15m/snapshot
3. limitless-btc-signal   → writes limitless/btc-15m/signal
4. limitless-btc-order    ← THIS SKILL: reads signal, fetches balance, places orders
```

Full chain trigger: _"Run BTC TA, fetch Limitless markets, generate signal, and place orders."_

---

## Credential & Authentication Notes

### What each key is used for

| Key | Used by | Purpose |
|-----|---------|---------|
| `LIMITLESS_API_KEY` | `http` tool (GET balance) + `limitless-cli` | Authenticates REST API requests via `X-API-Key` header |
| `LIMITLESS_PRIVATE_KEY` | `limitless-cli` only | Signs order payloads with EIP-712 (on-chain Base network) |

**Why `limitless-cli` is required for orders:** The `POST /orders` body must contain an EIP-712 cryptographic signature derived from `LIMITLESS_PRIVATE_KEY`. The `http` tool can pass headers and JSON but cannot perform elliptic curve signing. `limitless-cli` handles signing internally and submits the complete signed order payload.

**The `http` tool is sufficient for balance fetch** (`GET /portfolio/trading/allowance`) because that endpoint only needs the `X-API-Key` header — no signing required.

### Setup

**Step 1 — Add both keys to your `.env` file:**

```bash
# .env (in your IronClaw working directory)
LIMITLESS_API_KEY=lmts_your_key_here
LIMITLESS_PRIVATE_KEY=0xYourConnectedWalletPrivateKey
```

**Which private key?**
Limitless Exchange is **non-custodial** — there is no separate "Limitless wallet". Use the private key of the **wallet you connected to the exchange** (e.g. the MetaMask wallet you used to sign in at limitless.exchange). This is the same wallet that:
- Holds your USDC balance on Base network
- Signed the initial login/connection to the exchange
- Will sign each order via EIP-712

To export from MetaMask: Account Details → Export Private Key (requires password).

> **Security warning:** Never commit `.env` to version control. Add `.env` to `.gitignore`. The private key grants full control of the connected wallet — treat it like a password.

IronClaw loads `.env` at startup (`bootstrap.rs`), so `limitless-cli` called via the `shell` tool will automatically inherit both variables. No additional `secrets set` commands are needed.

**Step 2 — Install `limitless-cli`:**

```bash
npm install -g @limitless/cli

# Verify:
limitless-cli --version
```

**Step 3 — Verify env is loaded (optional check):**

```bash
# Ask IronClaw to run:
shell: echo $LIMITLESS_API_KEY
# Should print your key (first few chars)
```
