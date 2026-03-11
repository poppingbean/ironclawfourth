---
name: limitless-btc-signal
version: 0.1.0
description: "Read BTC multi-timeframe TA snapshots (from binance-btc-ta memory) and active Limitless Exchange 15m BTC markets (from limitless-btc-markets), then apply a multi-timeframe signal scoring model to output a YES/NO recommendation for each open prediction market."
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
  max_context_tokens: 2500
---

# Limitless BTC 15m Signal — YES/NO from Multi-Timeframe TA

Use this skill to produce a YES or NO recommendation for each active BTC 15-minute prediction market on Limitless Exchange, grounded in multi-timeframe technical analysis stored by the `binance-btc-ta` skill.

**Prerequisites:** Run `binance-btc-ta` first (or have recent memory entries), and have Limitless markets available. This skill reads from memory — no extra Binance API calls.

---

## Mandatory Execution Flow

Run every step in order. Do not skip.

---

### Step 1 — Load TA snapshots from memory

Read all four timeframe snapshots written by `binance-btc-ta`:

```
memory_read("ta/btc/4h")
memory_read("ta/btc/1h")
memory_read("ta/btc/15m")
memory_read("ta/btc/5m")
memory_read("ta/btc/summary")
```

If any key is missing or older than 30 minutes, note it and instruct the user to run `binance-btc-ta` first. Continue with available data if at least 3 of 4 timeframes are present.

---

### Step 2 — Load active Limitless 15m BTC markets

Try memory first (written by `limitless-btc-markets`):

```
memory_read("limitless/btc-15m/snapshot")
```

If missing or stale (older than 5 minutes), re-fetch live:

```
http tool:
  method: GET
  url: https://api.limitless.exchange/markets/active/2
```

Filter for markets with `15m`, `15min`, or `15-minute` in title/slug. Extract per market:
- `marketId`, `title`, `slug`
- `yes_price` = `outcomes[0].price` (probability 0.00–1.00)
- `no_price` = `outcomes[1].price`
- `volume24h`, `liquidity`

---

### Step 3 — Determine directional bias via timeframe cascade

Use the four TA snapshots to score directional bias **LONG (BUY) or SHORT (SELL)** on a 10-point scale. This maps directly to Limitless **YES (price goes up)** or **NO (price goes down)**.

#### Timeframe weights

| TF | Weight | Role |
|----|--------|------|
| 4h | Heavy | Primary trend anchor |
| 1h | Medium | Confirmation |
| 15m | Light | Entry timing |
| 5m | Tiebreaker | Short-term momentum only |

Require **≥ 2 of 3 primary timeframes** (4h, 1h, 15m) to agree. If split 2-1, 5m breaks the tie. If still split — **SKIP** (no bet).

#### Signal scoring (award +1 per condition true in the signal direction)

| # | Condition | Direction |
|---|-----------|-----------|
| 1 | RSI(14) on 1h < 40 | LONG |
| 1 | RSI(14) on 1h > 60 | SHORT |
| 2 | MACD histogram on 1h trending in signal direction (positive slope for LONG) | both |
| 3 | EMA(12) > EMA(26) on 15m | LONG |
| 3 | EMA(12) < EMA(26) on 15m | SHORT |
| 4 | current_price > SMA(50) on 4h | LONG |
| 4 | current_price < SMA(50) on 4h | SHORT |
| 5 | BB position on 1h < 0.25 (near lower band) | LONG |
| 5 | BB position on 1h > 0.75 (near upper band) | SHORT |
| 6 | RSI(14) on 15m < 45 | LONG |
| 6 | RSI(14) on 15m > 55 | SHORT |
| 7 | MACD histogram on 4h positive | LONG |
| 7 | MACD histogram on 4h negative | SHORT |
| 8 | Volume ratio on 15m > 1.3 AND direction matches | both |
| 9 | RSI(14) on 4h < 50 (not overbought) | LONG |
| 9 | RSI(14) on 4h > 50 (not oversold for SHORT) | SHORT |
| 10 | 5m MACD histogram matches signal direction | both |

**Count the score for LONG** and the score for **SHORT** independently. Use the higher-scoring direction. If both score < 5 — **SKIP**.

**Minimum score to generate a recommendation: 5 / 10.**

#### Hard overrides (apply after scoring)

| Condition | Override |
|-----------|----------|
| RSI(14) on any TF > 80 | Force SHORT regardless of score |
| RSI(14) on any TF < 20 | Force LONG regardless of score |
| BB squeeze on 15m (`bb_width < 0.015`) | SKIP — breakout direction unknown |
| Volume ratio on 15m < 0.4 (dead market) | SKIP — no momentum |

---

### Step 4 — Map signal to YES/NO per market

For each active Limitless 15m BTC market:

1. Parse the **strike price** from the market title (e.g., "BTC above $97,000 in 15m" → strike = 97,000)
2. Compare strike to `current_price` from the 15m TA snapshot:
   - If strike < current_price: market resolves YES if BTC stays above → signal LONG = **bet YES**, signal SHORT = **bet NO**
   - If strike > current_price: market resolves YES if BTC rises to target → signal LONG + strong momentum = **bet YES**, else = **bet NO**
   - If strike ≈ current_price (within 0.3%): treat as neutral zone — **reduce confidence by 1 point**

3. Assign confidence tier:

| Score | Tier | Recommendation weight |
|-------|------|-----------------------|
| 9–10 | High | Strong bet |
| 7–8 | Medium | Moderate bet |
| 5–6 | Low | Small/cautious bet |
| < 5 | — | SKIP |

---

### Step 5 — Output the recommendation table

Present results as:

```
## BTC 15m Limitless Signal — {timestamp} UTC
Directional bias: LONG | Score: 7/10

| Market | Strike | Current | Gap% | Rec | Confidence | YES price | NO price |
|--------|--------|---------|------|-----|------------|-----------|----------|
| BTC above $97,000 in 15m | $97,000 | $96,800 | -0.21% | YES | Medium (7) | 0.61 | 0.39 |
| BTC above $97,500 in 15m | $97,500 | $96,800 | -0.72% | NO  | Medium (7) | 0.28 | 0.72 |
| BTC above $96,500 in 15m | $96,500 | $96,800 | +0.31% | YES | Medium (7) | 0.74 | 0.26 |

Signal basis:
- 4h: Price above SMA50 ✓ | MACD positive ✓ | RSI 54
- 1h: RSI 42 ✓ | MACD bullish histogram ✓ | BB pos 0.31 (neutral)
- 15m: EMA12>EMA26 ✓ | RSI 47 ✓ | Vol ratio 1.4 ✓
- 5m: MACD bullish ✓

Score breakdown: LONG 7 / SHORT 3
```

Always include the signal basis so the user can verify the reasoning.

---

### Step 6 — Store recommendation in memory

```
memory_write:
  key: "limitless/btc-15m/signal"
  content: <full recommendation JSON with timestamp, score, direction, per-market YES/NO>
```

JSON structure:
```json
{
  "fetched_at": "2026-03-11T14:00:00Z",
  "direction": "LONG",
  "score": 7,
  "confidence": "Medium",
  "markets": [
    {
      "marketId": "...",
      "title": "BTC above $97,000 in 15m",
      "strike": 97000,
      "current_price": 96800,
      "gap_pct": -0.21,
      "recommendation": "YES",
      "yes_price": 0.61,
      "no_price": 0.39
    }
  ]
}
```

---

## Signal Rules (non-negotiable)

- **Score < 5 → always SKIP.** Do not force a recommendation.
- **BB squeeze on 15m → always SKIP.** Direction unknown during consolidation.
- **Stale TA data (> 30 min) → warn user and recommend re-running `binance-btc-ta` before betting.**
- **Never recommend a market with liquidity < $500** — price impact too high.
- **Never recommend a market with YES price between 0.45–0.55 AND score < 7** — too close to call.
- **Bollinger Bands alone never justify a signal.** Always require ≥ 2 other indicators agreeing.

---

## Scheduled Routine Setup

To run this skill automatically every 15 minutes at T+4 minutes (fires at :04, :19, :34, :49 of every hour — 2 minutes after `limitless-markets-15m` completes), create the following routine once:

> _"Set up the limitless-signal-15m routine"_

IronClaw will call `routine_create` with these exact parameters:

```
routine_create:
  name: "limitless-signal-15m"
  description: "Read BTC TA and Limitless market snapshots from memory, compute YES/NO signal, store result."
  trigger_type: "cron"
  schedule: "0 4,19,34,49 * * * *"
  action_type: "full_job"
  cooldown_secs: 840
  tool_permissions:
    - memory_read
    - memory_write
  prompt: |
    Compute a YES/NO trading signal for active Limitless BTC 15m markets using stored TA data.
    1. Read all four TA timeframes from memory:
       memory_read("ta/btc/4h")
       memory_read("ta/btc/1h")
       memory_read("ta/btc/15m")
       memory_read("ta/btc/5m")
       If any key is missing or fetched_at is older than 30 minutes — abort and log the stale key.
    2. Read the active Limitless market snapshot:
       memory_read("limitless/btc-15m/snapshot")
       If missing or fetched_at is older than 5 minutes — abort and log.
    3. Score directional bias LONG vs SHORT (0–10) using the multi-timeframe cascade:
       - 4h (heavy): SMA50 trend, MACD histogram, RSI direction
       - 1h (medium): RSI level, MACD histogram slope, BB position, EMA12 vs EMA26
       - 15m (light): EMA cross, RSI level, volume ratio
       - 5m (tiebreaker): MACD histogram direction
       Require ≥ 2 of 3 primary TFs (4h/1h/15m) to agree. Score < 5 or split → SKIP all markets.
    4. Apply hard overrides:
       - RSI > 80 on any TF → force SHORT
       - RSI < 20 on any TF → force LONG
       - BB squeeze (bb_width < 0.015) on 15m → SKIP
       - Volume ratio on 15m < 0.4 → SKIP
    5. For each market in the snapshot, compare strike price to current 15m close:
       - Determine YES or NO based on directional bias and strike vs current price gap.
       - Skip markets with liquidity < $500.
       - Skip markets with YES price 0.45–0.55 if score < 7.
    6. Store the signal to memory:
       memory_write("limitless/btc-15m/signal", <JSON: fetched_at, direction, score, confidence, markets[]>)
    7. Do not output a report — this is a background routine. Only log if SKIP or error.
```

**Cron field reference (6-field format):**

```
0   4,19,34,49  *  *  *  *
│   │           │  │  │  └── weekday (any)
│   │           │  │  └───── month (any)
│   │           │  └──────── day (any)
│   │           └─────────── hour (any)
│   └─────────────────────── minute (4, 19, 34, 49)
└─────────────────────────── second (0)
```

**Full timing chain:**

```
:00:15  btc-ta-15m            → ta/btc/{5m,15m,1h,4h} written        (~30s)
:02:00  limitless-markets-15m → limitless/btc-15m/snapshot written    (~20s)
:04:00  limitless-signal-15m  → limitless/btc-15m/signal written      (~15s)
          ↑ signal ready ~4m 15s after candle close — well before the next 15m window
```

To verify:
```
routine_list
```

To manually trigger:
```
routine_fire name="limitless-signal-15m"
```

---

## Skill Chain

This skill is designed to run **after** both upstream skills:

```
1. binance-btc-ta         → fetches OHLCV, computes metrics, writes ta/btc/{5m,15m,1h,4h}
2. limitless-btc-markets  → fetches active 15m markets, writes limitless/btc-15m/snapshot
3. limitless-btc-signal   ← THIS SKILL: reads both, outputs YES/NO per market
```

To run the full chain manually, ask: _"Run binance BTC TA, then fetch Limitless 15m markets, then give me the signal."_
