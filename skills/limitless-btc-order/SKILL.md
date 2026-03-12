---
name: limitless-btc-order
version: 0.2.0
description: "Read the YES/NO signal from memory and place orders on Limitless Exchange via the built-in order tool. Requires LIMITLESS_API_KEY, LIMITLESS_PRIVATE_KEY, and limitless-cli on PATH."
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
fetches live USDC balance, calculates order size (10% of balance), and places
orders via `limitless-cli`.

```
limitless_place_orders
```

To test without placing real orders, use `dry_run`:

```
limitless_place_orders dry_run=true
```

## Prerequisites

`limitless/btc-15m/signal` must have been written recently (signal must be
< 10 min old). If the tool errors with a stale-signal message, run the upstream
signal step first.

## Guards enforced by the tool

- Signal score < 5 → skipped automatically
- Signal older than 10 minutes → aborts
- Order size < $1.00 → aborts
- Total exposure > $50 without prior approval → blocked (ask IronClaw
  _"Approve Limitless orders above $50"_ once to pre-authorize)
- Market liquidity < 3× order size → that market skipped

## Scheduled routine

To run automatically every 15 minutes at T+8 minutes, create the routine once:

```
routine_create:
  name: "limitless-order-15m"
  description: "Read YES/NO signal from memory, fetch live USDC balance, place orders on Limitless Exchange via limitless-cli."
  trigger_type: "cron"
  schedule: "0 8,23,38,53 * * * *"
  action_type: "full_job"
  cooldown_secs: 840
  tool_permissions:
    - limitless_place_orders
  prompt: |
    Call limitless_place_orders. If the tool returns status "skipped", output
    only: "Skipping — {reason}" and stop. Background routine — no report output
    unless an order is placed or an error occurs.
```

## Full timing chain

```
:00:15  limitless-markets-15m      → limitless/btc-15m/snapshot
:02:00  limitless-signal-15m   → candles/btc/{5m,15m,1h} + limitless/btc-15m/signal
:08:00  limitless-order-15m        → orders placed          ← this step
```
