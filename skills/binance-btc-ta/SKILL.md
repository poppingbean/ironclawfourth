---
name: binance-btc-ta
version: 0.3.0
description: "Fetch raw BTC OHLCV candlestick data from Binance for 5m, 15m, and 1h timeframes and store to memory. No technical analysis is computed — raw candles are the input for the LLM signal step."
activation:
  keywords:
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
    - "btc.*(5m|15m|1h)"
    - "binance.*(btc|bitcoin)"
    - "btc.*(candle|kline|ohlcv)"
    - "fetch.*btc.*(data|candle)"
  tags:
    - trading
    - btc
    - binance
    - ohlcv
    - candles
  max_context_tokens: 400
---

# Binance BTC Raw Candle Fetch

Call the `btc_fetch_candles` tool. It fetches the last 100 BTC/USDT OHLCV
candles from Binance for 5m, 15m, and 1h concurrently, and stores the raw
data to memory. No technical indicators are computed at this step.

```
btc_fetch_candles
```

The tool stores raw candles to `candles/btc/5m`, `candles/btc/15m`, and
`candles/btc/1h`. Present the returned summary (counts stored per timeframe)
to the user. The LLM performs all analysis in the signal step.

**Important:** The last candle per timeframe is the currently forming candle.
The second-to-last is the most recent confirmed closed candle.

## Error handling

- If the tool returns `errors` listing failed timeframes, report them to the
  user. Partial success (some timeframes stored) is acceptable.
- Do not retry individual timeframes manually — call `btc_fetch_candles` again
  if a full refresh is needed.

## Scheduled routine

To run automatically every 15 minutes at T+2 minutes, create the routine once:

```
routine_create:
  name: "binance-btc-candles-15m"
  description: "Fetch raw BTC OHLCV candles from Binance for 5m/15m/1h and store to memory."
  trigger_type: "cron"
  schedule: "0 2,17,32,47 * * * *"
  action_type: "full_job"
  cooldown_secs: 840
  prompt: |
    Call btc_fetch_candles. When the tool returns successfully, output exactly:
    "done". If the tool reports errors for any timeframe, output the error details.
```

## Full timing chain

```
:00:15  limitless-markets-15m → limitless/btc-15m/snapshot
:02:00  binance-btc-candles-15m → candles/btc/{5m,15m,1h}
:04:00  limitless-signal-15m  → limitless/btc-15m/signal
:06:00  limitless-order-15m   → orders placed
```
