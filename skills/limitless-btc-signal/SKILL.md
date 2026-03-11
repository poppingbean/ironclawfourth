---
name: limitless-btc-signal
version: 0.2.0
description: "Read raw BTC candles and active Limitless Exchange 15m markets from memory, perform multi-timeframe technical analysis, and produce a YES/NO decision with USDC order quantity for each open prediction market."
activation:
  keywords:
    - limitless signal
    - btc prediction signal
    - yes or no btc
    - btc 15m signal
    - should i bet yes
    - limitless analysis
    - btc prediction
    - signal limitless
    - analyze limitless
    - ta signal limitless
  patterns:
    - "limitless.*(yes|no|signal|bet|predict)"
    - "btc.*15m.*(signal|predict|yes|no)"
    - "ta.*(limitless|prediction|market)"
    - "(yes|no).*limitless.*btc"
    - "analyze.*limitless.*market"
  tags:
    - trading
    - btc
    - limitless
    - prediction
    - signal
    - technical-analysis
  max_context_tokens: 2000
---

# Limitless BTC 15m Signal — LLM Multi-Timeframe Analysis

Read the raw candle data and the active market snapshot from memory, then
perform your own technical analysis and produce a YES/NO decision with a USDC
order quantity for each market.

## Step 1 — Load data from memory

Read the following memory keys:

```
memory_read: candles/btc/5m
memory_read: candles/btc/15m
memory_read: candles/btc/1h
memory_read: candles/btc/4h
memory_read: limitless/btc-15m/snapshot
```

Each candle object has fields: `t` (open_time ms), `o` (open), `h` (high),
`l` (low), `c` (close), `v` (volume BTC), `qv` (quote volume USDT),
`ct` (close_time ms). Use the **second-to-last** candle per timeframe as the
most recent confirmed close.

Abort with a clear error if any key is missing or older than 10 minutes.

## Step 2 — Perform technical analysis

For each timeframe (5m, 15m, 1h, 4h) compute from the raw candles:

- **Trend**: SMA-20, SMA-50 — is price above or below each? Are they aligned?
- **Momentum**: EMA-12 vs EMA-26, MACD direction (histogram positive/negative,
  divergence/convergence)
- **Mean reversion**: RSI-14 — overbought (>70) or oversold (<30)?
- **Volatility**: Bollinger Band position (above middle = bullish pressure,
  below = bearish)
- **Volume**: Current candle volume vs 20-period average — is the move
  confirmed by volume?

Score the multi-timeframe picture:
- Each aligned bullish signal = +1, bearish signal = -1, neutral = 0
- Sum the scores across all timeframes
- Score ≥ +4 → strong YES bias; ≤ -4 → strong NO bias; -3 to +3 → mixed

## Step 3 — Decision per market

For each market in `limitless/btc-15m/snapshot`:

1. Parse the strike price from the market title (e.g. "BTC above $97,000 in 15m").
2. Compare strike to current 15m close price.
3. Apply the multi-timeframe signal:
   - If the signal favors price going UP and the market is a YES-above-strike
     bet → decision = YES
   - If the signal favors price going DOWN or strike is far above current price
     → decision = NO
   - If signal is mixed (score -3 to +3) → SKIP (do not place order)
4. Calculate USDC quantity:
   - Base size = 10% of available USDC balance (read from context or estimate
     $10 as default if balance is unknown).
   - Scale by conviction: score ≥ 6 → 100%, score 4-5 → 75%, score 3 → 50%.
   - Minimum $1.00; if below, SKIP the market.
   - Maximum $50.00 per market without prior approval.

## Step 4 — Write signal to memory

Write the result to `limitless/btc-15m/signal`:

```json
{
  "computed_at": "<ISO timestamp>",
  "btc_price_15m": <last confirmed 15m close>,
  "score": <integer -10 to +10>,
  "markets": [
    {
      "market_id": "<id>",
      "title": "<title>",
      "strike": <price>,
      "decision": "YES|NO|SKIP",
      "reason": "<brief explanation>",
      "usdc_quantity": <float or null>
    }
  ]
}
```

## Step 5 — Present results

Show a table with columns: Market, Strike, Decision, USDC, Reason.

## Scheduled routine

To run automatically every 15 minutes at T+4 minutes, create the routine once:

```
routine_create:
  name: "limitless-signal-15m"
  description: "Read raw candles and market snapshot, perform LLM TA, write YES/NO signal to memory."
  trigger_type: "cron"
  schedule: "0 4,19,34,49 * * * *"
  action_type: "full_job"
  cooldown_secs: 840
  prompt: |
    Read candles/btc/5m, candles/btc/15m, candles/btc/1h, candles/btc/4h and
    limitless/btc-15m/snapshot from memory. Perform multi-timeframe technical
    analysis (SMA, EMA, MACD, RSI, Bollinger Bands, volume) and compute a
    YES/NO decision with USDC quantity for each active market. Write the result
    to limitless/btc-15m/signal. Do not output a report — background routine.
    Log only if SKIP or error.
```

## Full timing chain

```
:00:15  limitless-markets-15m      → limitless/btc-15m/snapshot
:02:00  binance-btc-candles-15m    → candles/btc/{5m,15m,1h,4h}
:04:00  limitless-signal-15m       → limitless/btc-15m/signal  ← this step
:06:00  limitless-order-15m        → orders placed
```
