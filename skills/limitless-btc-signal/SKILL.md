---
name: limitless-btc-signal
version: 0.7.0
description: "Fetch BTC/USDT OHLCV from Binance (5m/15m/1h/4h), compute multi-timeframe TA indicators, and produce a per-market YES/NO decision for each active Limitless Exchange prediction market."
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
  max_context_tokens: 800
---

# Limitless BTC 15m — Signal

Two tools, in order:

## Step 1 — Fetch candles + compute TA

```
btc_fetch_ta
```

Fetches 5m/15m/1h/4h OHLCV from Binance, computes SMA/EMA/MACD/RSI/BB/volume
indicators for each timeframe, and stores snapshots to `ta/btc/{5m,15m,1h,4h}`.

If all timeframes fail, stop and report the errors.

## Step 2 — Compute YES/NO signal per market

```
limitless_compute_signal
```

Reads the TA snapshots and the market snapshot from `limitless/btc-15m/snapshot`.
For each active market, compares the current BTC price to the market strike price
and combines that gap with the TA momentum score to decide YES or NO:

Every market always gets YES or NO — never SKIP:

- Price comfortably above strike (>1%) + not strongly bearish → **YES**
- Price comfortably below strike (<-1%) + not strongly bullish → **NO**
- Price near strike: follows net TA momentum direction; tie at exact strike → **NO**
- BB squeeze or very low volume: picks the higher raw TA score; tie → **NO** (conservative)
- **Weak YES override**: if `yes_score < 5` AND `(yes_score − no_score) ≤ 1`, force **NO** — bullish conviction too weak regardless of gap

Writes the result to `limitless/btc-15m/signal`:

```json
{
  "computed_at": "<ISO timestamp>",
  "btc_price_15m": <float>,
  "score": <net TA score -10 to +10>,
  "yes_score": <bullish indicators count>,
  "no_score": <bearish indicators count>,
  "markets": [
    {
      "market_id": "<id>",
      "title": "<title>",
      "slug": "<slug>",
      "strike": <float>,
      "current_price": <float>,
      "gap_pct": <float>,
      "decision": "YES|NO",
      "reason": "<explanation>",
      "yes_price": <float>,
      "no_price": <float>,
      "liquidity": <float>
    }
  ]
}
```

## Scheduled routine

Create once at T+2 minutes:

```
routine_create:
  name: "limitless-signal-15m"
  description: "Fetch BTC TA from Binance, compute YES/NO per prediction market, write signal to memory."
  trigger_type: "cron"
  schedule: "0 2,17,32,47 * * * *"
  action_type: "full_job"
  cooldown_secs: 840
  prompt: |
    Call btc_fetch_ta, then call limitless_compute_signal. Output exactly: "done".
```

## Full timing chain

```
:00:15  limitless-markets-15m  → limitless/btc-15m/snapshot
:02:00  limitless-signal-15m   → ta/btc/{5m,15m,1h,4h} + limitless/btc-15m/signal
:10:00  limitless-order-15m    → orders placed
```
