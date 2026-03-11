---
name: binance-btc-ta
version: 0.1.0
description: "Fetch BTC OHLCV candlestick data from Binance for 5m, 15m, 1h, and 4h timeframes. Compute technical analysis metrics (SMA, EMA, RSI, MACD, Bollinger Bands, volume) and store results in memory as input for trading decisions or Limitless market analysis."
activation:
  keywords:
    - binance btc
    - btc candles
    - btc ohlcv
    - technical analysis btc
    - btc ta
    - btc indicators
    - market metrics btc
    - btc candlestick
    - fetch btc data
    - btc price data
  patterns:
    - "btc.*(5m|15m|1h|4h)"
    - "binance.*(btc|bitcoin)"
    - "technical.*analysis.*btc"
    - "(sma|ema|rsi|macd|bollinger).*btc"
    - "market.*metrics.*btc"
  tags:
    - trading
    - btc
    - binance
    - technical-analysis
    - ohlcv
    - indicators
  max_context_tokens: 2000
---

# Binance BTC Technical Analysis — Multi-Timeframe Data Fetcher

Use this skill to fetch live BTC/USDT candlestick data from Binance for 5m, 15m, 1h, and 4h intervals, compute TA metrics from the OHLCV data, and store everything in memory for downstream analysis (e.g., Limitless Exchange signal generation).

## API Overview

- **Base URL**: `https://api.binance.com`
- **Endpoint**: `GET /api/v3/klines`
- **Authentication**: None required (public market data)
- **Rate limit**: 1200 requests/min weight; each klines call costs 2 weight

## Timeframe Targets

| Interval | Key | Candle limit | Purpose |
|----------|-----|--------------|---------|
| 5 minutes | `5m` | 100 candles | Short-term momentum |
| 15 minutes | `15m` | 100 candles | Limitless 15m market alignment |
| 1 hour | `1h` | 100 candles | Intraday trend |
| 4 hours | `4h` | 100 candles | Swing trend context |

## Step-by-Step Procedure

### Step 1 — Fetch all four timeframes

Make four sequential `http` tool calls (one per interval). Use `symbol=BTCUSDT` and `limit=100` for all.

**5m candles:**
```
http tool:
  method: GET
  url: https://api.binance.com/api/v3/klines?symbol=BTCUSDT&interval=5m&limit=100
```

**15m candles:**
```
http tool:
  method: GET
  url: https://api.binance.com/api/v3/klines?symbol=BTCUSDT&interval=15m&limit=100
```

**1h candles:**
```
http tool:
  method: GET
  url: https://api.binance.com/api/v3/klines?symbol=BTCUSDT&interval=1h&limit=100
```

**4h candles:**
```
http tool:
  method: GET
  url: https://api.binance.com/api/v3/klines?symbol=BTCUSDT&interval=4h&limit=100
```

### Step 2 — Parse the OHLCV response

Each response is an array of arrays. Map each row as:

| Index | Field | Type |
|-------|-------|------|
| 0 | Open time (ms epoch) | integer |
| 1 | Open | float string |
| 2 | High | float string |
| 3 | Low | float string |
| 4 | Close | float string |
| 5 | Volume (BTC) | float string |
| 6 | Close time (ms epoch) | integer |
| 7 | Quote asset volume (USDT) | float string |
| 8 | Number of trades | integer |

Convert all price/volume strings to floats before computing metrics.

### Step 3 — Compute TA metrics per timeframe

For each timeframe's 100 candles, compute the following. All calculations use **close prices** unless noted.

#### Price snapshot
- `current_price` = close of last candle
- `open_price` = open of last candle
- `high` = highest high in last 20 candles
- `low` = lowest low in last 20 candles
- `change_pct` = ((current_price - close[0]) / close[0]) * 100

#### Simple Moving Averages (SMA)
- `sma_20` = mean of last 20 closes
- `sma_50` = mean of last 50 closes

#### Exponential Moving Averages (EMA)
Use smoothing factor `k = 2 / (period + 1)`.
- `ema_12` — 12-period EMA (seed from SMA-12 of first 12 candles, then apply EMA formula forward)
- `ema_26` — 26-period EMA (seed from SMA-26)

#### MACD
- `macd_line` = ema_12 − ema_26
- `signal_line` = 9-period EMA of the macd_line series
- `histogram` = macd_line − signal_line

#### RSI (14-period)
1. Compute 14 consecutive close-to-close changes
2. Separate into gains (positive changes) and losses (absolute negative changes)
3. `avg_gain` = mean of gains, `avg_loss` = mean of losses
4. For candles 15 onward use Wilder smoothing:
   `avg_gain = (prev_avg_gain * 13 + current_gain) / 14`
5. `rs` = avg_gain / avg_loss
6. `rsi` = 100 − (100 / (1 + rs))

#### Bollinger Bands (20-period, 2 std dev)
- `bb_middle` = sma_20
- `bb_std` = standard deviation of last 20 closes
- `bb_upper` = bb_middle + 2 × bb_std
- `bb_lower` = bb_middle − 2 × bb_std
- `bb_width` = (bb_upper − bb_lower) / bb_middle
- `bb_position` = (current_price − bb_lower) / (bb_upper − bb_lower)  *(0 = at lower, 1 = at upper)*

#### Volume
- `volume_avg_20` = mean of last 20 candle volumes (BTC)
- `volume_current` = volume of last candle
- `volume_ratio` = volume_current / volume_avg_20  *(>1 = above average)*
- `quote_volume_24h` = sum of quote asset volume (USDT) for last 96 candles of 15m, or last 6 candles of 4h

### Step 4 — Derive signals

After computing metrics for all timeframes, derive simple directional signals:

| Signal | Condition | Value |
|--------|-----------|-------|
| Trend | current_price > sma_50 | `bullish` / `bearish` |
| Momentum | rsi > 55 | `strong` / rsi < 45 → `weak` / else `neutral` |
| MACD cross | histogram > 0 and prev histogram ≤ 0 | `bullish_cross` |
| MACD cross | histogram < 0 and prev histogram ≥ 0 | `bearish_cross` |
| BB squeeze | bb_width < 0.02 | `squeeze` |
| Overbought | rsi > 70 | `true` |
| Oversold | rsi < 30 | `true` |
| Volume spike | volume_ratio > 1.5 | `true` |

### Step 5 — Store results in memory

Write each timeframe's metrics to memory for downstream use:

```
memory_write:
  key: "ta/btc/5m"
  content: <JSON of 5m metrics and signals>

memory_write:
  key: "ta/btc/15m"
  content: <JSON of 15m metrics and signals>

memory_write:
  key: "ta/btc/1h"
  content: <JSON of 1h metrics and signals>

memory_write:
  key: "ta/btc/4h"
  content: <JSON of 4h metrics and signals>

memory_write:
  key: "ta/btc/summary"
  content: <consolidated multi-timeframe summary with timestamp>
```

Use ISO 8601 timestamps. Include `fetched_at` (UTC) in every stored object.

### Step 6 — Present the summary

Output a compact multi-timeframe table:

```
## BTC/USDT Technical Analysis — {timestamp} UTC

| TF  | Price      | SMA20      | SMA50      | RSI  | MACD      | BB pos | Vol ratio | Trend    |
|-----|------------|------------|------------|------|-----------|--------|-----------|----------|
| 5m  | $XX,XXX.XX | $XX,XXX.XX | $XX,XXX.XX | XX.X | +X.XX     | 0.XX   | X.XXx     | bullish  |
| 15m | ...        | ...        | ...        | ...  | ...       | ...    | ...       | ...      |
| 1h  | ...        | ...        | ...        | ...  | ...       | ...    | ...       | ...      |
| 4h  | ...        | ...        | ...        | ...  | ...       | ...    | ...       | ...      |

Signals: [RSI overbought on 15m] [MACD bullish cross on 1h] [Volume spike on 5m]
Stored to memory: ta/btc/{5m,15m,1h,4h,summary}
```

## Integration with Limitless BTC Markets

After running this skill, the stored `ta/btc/15m` metrics can be read by the `limitless-btc-markets` skill (or manually) to contextualize prediction market prices against real-time TA signals:

```
memory_read key="ta/btc/15m"   → current RSI, MACD, trend
→ compare against Limitless 15m YES/NO prices for alpha
```

## Error Handling

- **Rate limit (HTTP 429)**: Wait and retry the failed interval; do not retry all four at once
- **Symbol not found (HTTP 400)**: Confirm `symbol=BTCUSDT` is correct; try `BTCBUSD` as fallback
- **Partial failure**: Store whichever timeframes succeeded, note failures in the summary
- **Empty candles array**: Report and skip that timeframe

## Scheduled Routine Setup

To run this skill automatically every 15 minutes at T+15 seconds (i.e., fires at :00:15, :15:15, :30:15, :45:15 of every hour), create the following routine once by asking IronClaw:

> _"Set up the btc-ta-15m routine"_

IronClaw will call `routine_create` with these exact parameters:

```
routine_create:
  name: "btc-ta-15m"
  description: "Fetch BTC OHLCV from Binance for 5m/15m/1h/4h, compute TA metrics, store to memory."
  trigger_type: "cron"
  schedule: "15 0,15,30,45 * * * *"
  action_type: "full_job"
  cooldown_secs: 840
  tool_permissions:
    - http
    - memory_write
  prompt: |
    Run the full BTC technical analysis fetch cycle:
    1. Fetch BTC/USDT klines from Binance for 5m, 15m, 1h, and 4h (limit=100 each):
       GET https://api.binance.com/api/v3/klines?symbol=BTCUSDT&interval=5m&limit=100
       GET https://api.binance.com/api/v3/klines?symbol=BTCUSDT&interval=15m&limit=100
       GET https://api.binance.com/api/v3/klines?symbol=BTCUSDT&interval=1h&limit=100
       GET https://api.binance.com/api/v3/klines?symbol=BTCUSDT&interval=4h&limit=100
    2. Parse OHLCV (index 0=openTime, 1=open, 2=high, 3=low, 4=close, 5=volume).
       Use candle[-2] (second-to-last) as the confirmed closed candle.
    3. Compute per timeframe: SMA20, SMA50, EMA12, EMA26, MACD (line/signal/histogram),
       RSI-14 (Wilder smoothing), Bollinger Bands (20-period 2σ: upper/middle/lower/width/position),
       volume_avg_20, volume_ratio, and directional signals (trend/momentum/BB squeeze/overbought/oversold/volume spike).
    4. Store each timeframe to memory:
       memory_write("ta/btc/5m", <JSON metrics>)
       memory_write("ta/btc/15m", <JSON metrics>)
       memory_write("ta/btc/1h", <JSON metrics>)
       memory_write("ta/btc/4h", <JSON metrics>)
       memory_write("ta/btc/summary", <consolidated JSON with fetched_at UTC timestamp>)
    5. Do not output a report — this is a background routine. Only log errors if a fetch fails.
```

**Cron field reference (6-field format):**

```
15  0,15,30,45  *  *  *  *
│   │           │  │  │  └── weekday (any)
│   │           │  │  └───── month (any)
│   │           │  └──────── day (any)
│   │           └─────────── hour (any)
│   └─────────────────────── minute (0, 15, 30, 45)
└─────────────────────────── second (15)
```

**Why T+15 seconds:** The 15-minute Binance candle closes exactly on the minute boundary. Waiting 15 seconds ensures the candle is fully settled and propagated to the API before fetching.

**`cooldown_secs: 840`** (14 minutes) prevents double-fires if the routine is manually triggered while the cron is live.

To verify the routine was created:
```
routine_list
```

To manually trigger a fetch immediately:
```
routine_fire name="btc-ta-15m"
```

## Notes

- Binance returns closes sorted oldest → newest; index `[-1]` (last element) is the **current forming candle** — use `[-2]` as the last **confirmed closed candle** for indicator accuracy
- All prices are in USDT
- The 15m data aligns directly with Limitless Exchange BTC 15m prediction market windows
