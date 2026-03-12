---
name: limitless-btc-signal
version: 0.4.0
description: "Fetch BTC/USDT OHLCV candles from Binance (5m/15m/1h), read active Limitless Exchange markets from memory, perform multi-timeframe technical analysis, and produce a YES/NO decision with USDC order quantity for each open prediction market."
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
    - binance btc
    - btc candles
    - btc ohlcv
    - fetch btc candles
    - btc candlestick
    - fetch btc data
    - btc price data
    - raw candles btc
    - btc klines
  patterns:
    - "limitless.*(yes|no|signal|bet|predict)"
    - "btc.*15m.*(signal|predict|yes|no)"
    - "ta.*(limitless|prediction|market)"
    - "(yes|no).*limitless.*btc"
    - "analyze.*limitless.*market"
    - "btc.*(5m|15m|1h)"
    - "binance.*(btc|bitcoin)"
    - "btc.*(candle|kline|ohlcv)"
    - "fetch.*btc.*(data|candle)"
  tags:
    - trading
    - btc
    - binance
    - limitless
    - ohlcv
    - candles
    - prediction
    - signal
    - technical-analysis
  max_context_tokens: 2000
---

# Limitless BTC 15m — Fetch Candles + Signal

Fetch fresh BTC candles from Binance, then read the active market snapshot and
produce a YES/NO decision for each open Limitless Exchange prediction market.

## Step 1 — Fetch candles from Binance

Call `btc_fetch_candles`. It fetches the last 100 BTC/USDT OHLCV candles for
5m, 15m, and 1h concurrently and stores them to memory.

```
btc_fetch_candles
```

Stores to: `candles/btc/5m`, `candles/btc/15m`, `candles/btc/1h`.

**If the tool returns errors for all timeframes**, stop and report the errors.
Partial success (some timeframes stored) is acceptable — continue.

**Important:** The last candle per timeframe is the currently forming candle.
The second-to-last is the most recent confirmed closed candle.

## Step 2 — Load market snapshot from memory

```
memory_read: limitless/btc-15m/snapshot
```

**If missing**, or if its `active` field is `false`, or its `markets` array is
empty, output only: `"Skipping — no active markets this cycle."` and stop.

## Step 3 — Perform technical analysis

Each candle has fields: `t` (open_time ms), `o` (open), `h` (high), `l` (low),
`c` (close), `v` (volume BTC), `qv` (quote volume USDT), `ct` (close_time ms).

For each timeframe (5m, 15m, 1h) compute from the raw candles:

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

## Step 4 — Decision per market

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

## Step 5 — Write signal to memory

Write the result to `limitless/btc-15m/signal`:

```json
{
  "computed_at": "<ISO timestamp>",
  "btc_price_15m": "<last confirmed 15m close>",
  "score": "<integer -10 to +10>",
  "markets": [
    {
      "market_id": "<id>",
      "title": "<title>",
      "strike": "<price>",
      "decision": "YES|NO|SKIP",
      "reason": "<brief explanation>",
      "usdc_quantity": "<float or null>"
    }
  ]
}
```

## Scheduled routine

Replaces both `binance-btc-candles-15m` and `limitless-signal-15m`. Create once
at T+2 minutes:

```
routine_create:
  name: "limitless-signal-15m"
  description: "Fetch BTC candles from Binance, perform LLM TA on 5m/15m/1h, write YES/NO signal to memory."
  trigger_type: "cron"
  schedule: "0 2,17,32,47 * * * *"
  action_type: "full_job"
  cooldown_secs: 840
  prompt: |
    Call btc_fetch_candles. If all timeframes fail, output the errors and stop.
    Read limitless/btc-15m/snapshot from memory. If missing or markets empty,
    output only: "Skipping — no active markets." and stop.
    Otherwise perform multi-timeframe TA (SMA, EMA, MACD, RSI, BB, volume) on
    5m/15m/1h candles, compute YES/NO/SKIP per market, write to
    limitless/btc-15m/signal. Output exactly: "done".
```

**Also delete** the now-redundant `binance-btc-candles-15m` routine.

## Full timing chain

```
:00:15  limitless-markets-15m  → limitless/btc-15m/snapshot
:02:00  limitless-signal-15m   → candles/btc/{5m,15m,1h} + limitless/btc-15m/signal
:05:00  limitless-order-15m    → orders placed
```
