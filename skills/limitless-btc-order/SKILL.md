---
name: limitless-btc-order
version: 0.5.0
description: "Read the YES/NO signal from memory and place orders on Limitless Exchange via direct HTTP API (EIP-712 signed, no CLI required)."
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
  max_context_tokens: 400
---

# Limitless BTC 15m — Order Execution

Call the `limitless_place_orders` tool. It reads the signal from memory,
fetches live USDC balance via BaseScan, calculates order size, and places
orders via direct HTTP API with EIP-712 signing (no CLI required).

```
limitless_place_orders
```

To test without placing real orders:

```
limitless_place_orders dry_run=true
```

## What the tool does

1. Reads `limitless/btc-15m/signal` — aborts if older than 10 minutes
2. Filters markets to those with `decision = YES` or `decision = NO`
3. Fetches live BTC price from Binance at order time (T+10) and compares to each market's strike price — flips decision if live price moved >2% against the signal
4. Fetches live USDC balance from BaseScan (on-chain)
5. Calculates order size: 10% of balance (strong conviction) or 3% (weak: `|yes_score - no_score| ≤ 2`)
6. Fallback if BaseScan unavailable: $6.00 (strong) or $2.00 (weak)
7. Places each order via the Limitless CLI

## Guards enforced by the tool

- Signal older than 10 minutes → aborts
- On-chain USDC balance = 0 → aborts
- Order size < $1.00 → aborts
- Total exposure > $50 without prior approval → blocked
- Market liquidity < 3× order size → that market skipped

## Credentials required (in `~/.ironclaw/.env`)

```
LIMITLESS_API_KEY=...
LIMITLESS_PRIVATE_KEY=0x...
LIMITLESS_WALLET_ADDRESS=0x...
BASESCAN_API_KEY=...
```

## Scheduled routine

```
routine_create:
  name: "limitless-order-15m"
  description: "Read YES/NO signal from memory, fetch live USDC balance, place orders on Limitless Exchange via HTTP API."
  trigger_type: "cron"
  schedule: "0 10,25,40,55 * * * *"
  action_type: "full_job"
  cooldown_secs: 840
  tool_permissions:
    - limitless_place_orders
  prompt: |
    Call limitless_place_orders immediately. If the tool returns "skipped",
    output only: "skipped". Otherwise output exactly: "done".
```

## Full timing chain

```
:00:15  limitless-markets-15m  → limitless/btc-15m/snapshot
:02:00  limitless-signal-15m   → ta/btc/{5m,15m,1h,4h} + limitless/btc-15m/signal
:10:00  limitless-order-15m    → orders placed  ← this step
```
