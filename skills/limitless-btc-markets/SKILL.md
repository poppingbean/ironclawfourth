---
name: limitless-btc-markets
version: 0.1.0
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
  max_context_tokens: 1500
---

# Limitless Exchange — BTC 15m Active Markets

Use this skill whenever the user asks about active BTC 15-minute prediction markets on Limitless Exchange.

## API Overview

- **Base URL**: `https://api.limitless.exchange`
- **Authentication**: None required for read operations (public API)
- **Protocol**: HTTPS only

## Step-by-Step Procedure

### Step 1 — Fetch active BTC markets (category 2)

Use category ID **2** to scope the request to the BTC/crypto category, minimizing payload size:

```
http tool:
  method: GET
  url: https://api.limitless.exchange/markets/active/2
```

Only fall back to the broader search endpoint if category 2 returns no 15m markets:

```
http tool:
  method: GET
  url: https://api.limitless.exchange/markets/search?query=btc+15m&limit=50
```

No headers or authentication needed.

### Step 2 — Filter for 15-minute resolution

From the response, filter markets where the title, description, or slug contains any of:
- `15m`
- `15min`
- `15 min`
- `15-minute`

Markets on Limitless Exchange use slug formats like `btc-above-XXXXX-15m` for 15-minute resolution.

### Step 3 — Extract and display key fields

For each matching market, extract and present:

| Field | JSON path | Description |
|-------|-----------|-------------|
| Title | `title` | Market question/name |
| Yes price | `outcomes[0].price` | Probability of YES (0.01–0.99) |
| No price | `outcomes[1].price` | Probability of NO |
| 24h Volume | `volume24h` | Trading volume last 24h (USDC) |
| Liquidity | `liquidity` | Available liquidity (USDC) |
| Open Interest | `openInterest` | Total open interest (USDC) |
| Market ID | `marketId` | Unique identifier |

### Step 4 — Format the output

Present results as a markdown table:

```
| Market | YES | NO | Volume 24h | Liquidity |
|--------|-----|----|------------|-----------|
| BTC above $X in 15m | 0.62 | 0.38 | $12,450 | $8,200 |
```

If no 15m markets are found, report this clearly and show the top BTC markets instead (any resolution).

### Step 5 — Optional: store results in memory

If the user wants to track or compare market data over time, use `memory_write` to save the current snapshot:

```
memory_write key="limitless/btc-15m/snapshot" content="<snapshot>"
```

## Error Handling

- **HTTP 4xx/5xx**: Report the status code and suggest retrying
- **Empty response**: Inform the user and try the search endpoint with a broader query (`?query=btc&limit=100`)
- **No 15m markets**: Show all active BTC markets and note that 15m markets may not be currently active

## Notes

- The Limitless Exchange resolves markets using **Pyth Network oracles** (on-chain price feeds)
- Markets run on the **Base network** (Chain ID 8453)
- Prices are denominated in **USDC**
- Trade types: **AMM** (Automated Market Maker) and **CLOB** (Central Limit Order Book)
- For placing orders or managing positions, an API key and EIP-712 private key signature are required (out of scope for this read-only skill)
