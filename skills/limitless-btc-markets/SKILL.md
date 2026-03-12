---
name: limitless-btc-markets
version: 0.2.0
description: "Query active BTC 15-minute prediction markets from Limitless Exchange using the public REST API. Use when asked about BTC 15m markets, current prediction prices, or Limitless Exchange market data."
activation:
  keywords:
    - btc 15m
    - bitcoin 15m
    - limitless exchange
    - limitless btc
    - btc market
    - bitcoin market
    - active markets btc
    - prediction market btc
    - 15 minute btc
    - btc prediction
  patterns:
    - "btc.*15m"
    - "limitless.*market"
    - "bitcoin.*15.*min"
    - "active.*market.*btc"
  tags:
    - trading
    - markets
    - btc
    - prediction
    - limitless
  max_context_tokens: 400
---

# Limitless Exchange — BTC 15m Active Markets

Call the `limitless_fetch_markets` tool. It fetches active BTC 15-minute
prediction markets from Limitless Exchange (category 2), filters for 15m
markets, and stores the snapshot to memory.

```
limitless_fetch_markets
```

The tool stores results to `limitless/btc-15m/snapshot` and returns the
filtered market list with YES/NO prices and liquidity. Present the results
as a table to the user.

## Error handling

- If the tool reports zero markets, tell the user and note that 15m markets
  may not be active at the moment.
- Do not make manual `http` calls — the tool handles the fallback search
  endpoint automatically.

## Scheduled routine

To run automatically every 15 minutes at T+15 seconds, create the routine once:

```
routine_create:
  name: "limitless-markets-15m"
  description: "Fetch active BTC 15m prediction markets from Limitless Exchange and store snapshot to memory."
  trigger_type: "cron"
  schedule: "15 0,15,30,45 * * * *"
  action_type: "full_job"
  cooldown_secs: 840
  prompt: |
    Call limitless_fetch_markets. Do not output a report — background routine.
    Log only if no markets found or fetch fails.
```

## Full timing chain

```
:00:15  limitless-markets-15m → limitless/btc-15m/snapshot
:02:00  binance-btc-ta        → candles/btc/{5m,15m,1h}
:05:00  limitless-signal-15m  → limitless/btc-15m/signal
:08:00  limitless-order-15m   → orders placed
```
