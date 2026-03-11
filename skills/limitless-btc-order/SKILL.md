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

Ask the user to confirm before proceeding if `order_size_usdc > 50.00`. For amounts ≤ $50 per order, proceed automatically.

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
- **Confirm before executing if total exposure > $50.** Calculate `N_orders × order_size_usdc` and ask if > $50.
- **Skip any market with liquidity < 3× order size.** Thin books cause excessive slippage.

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
