//! Prediction tools for the Limitless Exchange BTC 15m trading pipeline.
//!
//! Moves all API calls, TA computation, signal scoring, and order placement
//! into native Rust so the LLM only needs to call one tool per step instead
//! of doing math and HTTP calls inside its context window.
//!
//! Pipeline:
//!   `btc_fetch_ta`             → fetch Binance OHLCV + compute indicators → ta/btc/*
//!   `limitless_fetch_markets`  → fetch active BTC 15m markets → limitless/btc-15m/snapshot
//!   `limitless_compute_signal` → score LONG/SHORT from memory → limitless/btc-15m/signal
//!   `limitless_place_orders`   → read signal + place orders via limitless-cli

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::context::JobContext;
use crate::error::WorkspaceError;
use crate::tools::tool::{ApprovalRequirement, Tool, ToolError, ToolOutput};
use crate::workspace::Workspace;

// ── Stored data structures ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TaSnapshot {
    interval: String,
    fetched_at: String,
    current_price: f64,
    open_price: f64,
    high_20: f64,
    low_20: f64,
    change_pct: f64,
    sma_20: f64,
    sma_50: f64,
    ema_12: f64,
    ema_26: f64,
    macd_line: f64,
    signal_line: f64,
    histogram: f64,
    prev_histogram: f64,
    rsi: f64,
    bb_upper: f64,
    bb_middle: f64,
    bb_lower: f64,
    bb_width: f64,
    bb_position: f64,
    volume_avg_20: f64,
    volume_current: f64,
    volume_ratio: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MarketEntry {
    market_id: String,
    title: String,
    slug: String,
    yes_price: f64,
    no_price: f64,
    volume_24h: f64,
    liquidity: f64,
}

// ── Tool 0: btc_fetch_candles ─────────────────────────────────────────────────

/// Fetch raw BTC/USDT OHLCV candlestick data for 5m, 15m, 1h, and 4h from
/// Binance and store to memory. No technical indicators are computed — the LLM
/// receives raw numbers and performs its own analysis in the signal step.
pub struct BtcFetchCandlesTool {
    workspace: Arc<Workspace>,
}

impl BtcFetchCandlesTool {
    pub fn new(workspace: Arc<Workspace>) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl Tool for BtcFetchCandlesTool {
    fn name(&self) -> &str {
        "btc_fetch_candles"
    }

    fn description(&self) -> &str {
        "Fetch raw BTC/USDT OHLCV candlestick data from Binance for 5m, 15m, and 1h \
         timeframes (last 100 confirmed candles each). Stores each timeframe to \
         candles/btc/{interval} in memory for the signal step. No indicators are \
         pre-computed — the LLM performs its own analysis. The second-to-last candle \
         per timeframe is the most recent confirmed close."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    async fn execute(
        &self,
        _params: serde_json::Value,
        _ctx: &JobContext,
    ) -> Result<ToolOutput, ToolError> {
        let start = std::time::Instant::now();
        let client = build_http_client()?;
        let fetched_at = Utc::now().to_rfc3339();

        // Fetch 3 timeframes concurrently (4h excluded — not needed for 15m signal)
        let (r5m, r15m, r1h) = tokio::join!(
            fetch_raw_candles(&client, "5m"),
            fetch_raw_candles(&client, "15m"),
            fetch_raw_candles(&client, "1h"),
        );

        let mut summary: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
        let mut errors: Vec<String> = Vec::new();

        for (interval, result) in [("5m", r5m), ("15m", r15m), ("1h", r1h)] {
            match result {
                Ok(rows) => {
                    let count = rows.as_array().map_or(0, |a| a.len());
                    // Store to workspace: candles/btc/{interval}
                    let payload = serde_json::json!({
                        "interval": interval,
                        "fetched_at": fetched_at,
                        "symbol": "BTCUSDT",
                        "note": "Second-to-last candle is the most recent confirmed close.",
                        "candles": rows,
                    });
                    let path = format!("candles/btc/{interval}");
                    self.workspace
                        .write(&path, &serde_json::to_string(&payload).unwrap_or_default())
                        .await
                        .map_err(|e| {
                            ToolError::ExecutionFailed(format!("Memory write {path}: {e}"))
                        })?;
                    summary.insert(
                        interval.to_string(),
                        serde_json::json!({ "candles_stored": count, "path": path }),
                    );
                }
                Err(e) => errors.push(format!("{interval}: {e}")),
            }
        }

        if summary.is_empty() {
            return Err(ToolError::ExecutionFailed(format!(
                "All Binance timeframes failed: {}",
                errors.join("; ")
            )));
        }

        Ok(ToolOutput::success(
            serde_json::json!({
                "fetched_at": fetched_at,
                "symbol": "BTCUSDT",
                "stored": summary,
                "errors": errors,
            }),
            start.elapsed(),
        ))
    }
}

/// Convert raw klines `[f64; 8]` to labeled JSON objects and return as a JSON array.
/// Fields: open_time (ms), open, high, low, close, volume (BTC), quote_volume (USDT), close_time (ms).
async fn fetch_raw_candles(
    client: &reqwest::Client,
    interval: &str,
) -> Result<serde_json::Value, String> {
    let klines = fetch_binance_klines(client, interval).await?;
    let rows: Vec<serde_json::Value> = klines
        .into_iter()
        .map(|k| {
            serde_json::json!({
                "t":  k[6] as i64,  // open_time ms
                "o":  k[0],         // open
                "h":  k[1],         // high
                "l":  k[2],         // low
                "c":  k[3],         // close
                "v":  k[4],         // volume BTC
                "qv": k[5],         // quote volume USDT
                "ct": k[7] as i64,  // close_time ms
            })
        })
        .collect();
    Ok(serde_json::Value::Array(rows))
}

// ── Tool 1: btc_fetch_ta ──────────────────────────────────────────────────────

/// Fetch BTC/USDT OHLCV from Binance for 5m/15m/1h/4h, compute TA indicators,
/// and store results to workspace memory. Eliminates 4 http calls and all
/// manual TA math from the LLM context window.
pub struct BtcFetchTaTool {
    workspace: Arc<Workspace>,
}

impl BtcFetchTaTool {
    pub fn new(workspace: Arc<Workspace>) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl Tool for BtcFetchTaTool {
    fn name(&self) -> &str {
        "btc_fetch_ta"
    }

    fn description(&self) -> &str {
        "Fetch BTC/USDT OHLCV from Binance for 5m, 15m, 1h, and 4h. \
         Compute SMA20/50, EMA12/26, MACD, RSI-14, and Bollinger Bands. \
         Store per-timeframe results to ta/btc/{5m,15m,1h,4h} and ta/btc/summary. \
         Returns a multi-timeframe summary."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    async fn execute(
        &self,
        _params: serde_json::Value,
        _ctx: &JobContext,
    ) -> Result<ToolOutput, ToolError> {
        let start = std::time::Instant::now();
        let client = build_http_client()?;
        let fetched_at = Utc::now().to_rfc3339();

        let (r5m, r15m, r1h, r4h) = tokio::join!(
            fetch_binance_klines(&client, "5m"),
            fetch_binance_klines(&client, "15m"),
            fetch_binance_klines(&client, "1h"),
            fetch_binance_klines(&client, "4h"),
        );

        let mut snapshots: Vec<TaSnapshot> = Vec::new();
        let mut errors: Vec<String> = Vec::new();

        for (interval, result) in [("5m", r5m), ("15m", r15m), ("1h", r1h), ("4h", r4h)] {
            match result {
                Ok(klines) => match compute_ta(&klines, interval, &fetched_at) {
                    Ok(snap) => {
                        let json = serde_json::to_string(&snap).map_err(|e| {
                            ToolError::ExecutionFailed(format!("Serialize error: {e}"))
                        })?;
                        self.workspace
                            .write(&format!("ta/btc/{interval}"), &json)
                            .await
                            .map_err(|e| {
                                ToolError::ExecutionFailed(format!(
                                    "Memory write ta/btc/{interval} failed: {e}"
                                ))
                            })?;
                        snapshots.push(snap);
                    }
                    Err(e) => errors.push(format!("{interval}: {e}")),
                },
                Err(e) => errors.push(format!("{interval}: {e}")),
            }
        }

        if snapshots.is_empty() {
            return Err(ToolError::ExecutionFailed(format!(
                "All timeframes failed: {}",
                errors.join("; ")
            )));
        }

        let summary = serde_json::json!({
            "fetched_at": fetched_at,
            "timeframes": snapshots.iter().map(|s| serde_json::json!({
                "interval": s.interval,
                "price": s.current_price,
                "rsi": s.rsi,
                "macd_histogram": s.histogram,
                "bb_width": s.bb_width,
                "bb_position": s.bb_position,
                "volume_ratio": s.volume_ratio,
                "trend": if s.current_price > s.sma_50 { "bullish" } else { "bearish" },
            })).collect::<Vec<_>>(),
            "errors": errors,
        });

        let summary_json = serde_json::to_string(&summary).map_err(|e| {
            ToolError::ExecutionFailed(format!("Serialize error: {e}"))
        })?;
        self.workspace
            .write("ta/btc/summary", &summary_json)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Memory write summary failed: {e}")))?;

        Ok(ToolOutput::success(summary, start.elapsed()))
    }
}

// ── Tool 2: limitless_fetch_markets ──────────────────────────────────────────

/// Fetch active BTC 15-minute prediction markets from Limitless Exchange.
/// Stores the filtered snapshot to limitless/btc-15m/snapshot.
pub struct LimitlessFetchMarketsTool {
    workspace: Arc<Workspace>,
}

impl LimitlessFetchMarketsTool {
    pub fn new(workspace: Arc<Workspace>) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl Tool for LimitlessFetchMarketsTool {
    fn name(&self) -> &str {
        "limitless_fetch_markets"
    }

    fn description(&self) -> &str {
        "Fetch active BTC 15-minute prediction markets from Limitless Exchange (category 2). \
         Filter for 15m markets. Store snapshot to limitless/btc-15m/snapshot. \
         Returns the list of markets with YES/NO prices and liquidity."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    async fn execute(
        &self,
        _params: serde_json::Value,
        _ctx: &JobContext,
    ) -> Result<ToolOutput, ToolError> {
        let start = std::time::Instant::now();
        let client = build_http_client()?;
        let fetched_at = Utc::now().to_rfc3339();

        let (markets, raw_items) = fetch_limitless_markets_15m(&client).await?;
        let count = markets.len();
        let raw_count = raw_items.len();
        let active = count > 0;

        let snapshot = serde_json::json!({
            "fetched_at": fetched_at,
            "active": active,
            "markets": markets,
        });
        let json = serde_json::to_string(&snapshot)
            .map_err(|e| ToolError::ExecutionFailed(format!("Serialize error: {e}")))?;
        self.workspace
            .write("limitless/btc-15m/snapshot", &json)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Memory write failed: {e}")))?;

        if !active {
            // Include a sample of the raw API response so the user can see
            // the actual field names if parse_market_entry is rejecting items.
            let raw_sample = raw_items.first().cloned().unwrap_or(serde_json::Value::Null);
            let raw_keys: Vec<String> = raw_sample
                .as_object()
                .map(|obj| obj.keys().cloned().collect())
                .unwrap_or_default();
            return Ok(ToolOutput::success(
                serde_json::json!({
                    "active": false,
                    "count": 0,
                    "raw_items_from_api": raw_count,
                    "fetched_at": fetched_at,
                    "note": if raw_count == 0 {
                        "API returned 0 items — no active markets this cycle."
                    } else {
                        "API returned items but none could be parsed. Check raw_sample_keys."
                    },
                    "raw_sample_keys": raw_keys,
                    "raw_sample": raw_sample,
                    "stored_at": "limitless/btc-15m/snapshot",
                }),
                start.elapsed(),
            ));
        }

        Ok(ToolOutput::success(
            serde_json::json!({
                "active": true,
                "count": count,
                "fetched_at": fetched_at,
                "markets": snapshot["markets"],
                "stored_at": "limitless/btc-15m/snapshot",
            }),
            start.elapsed(),
        ))
    }
}

// ── Tool 3: limitless_compute_signal ─────────────────────────────────────────

/// Read TA snapshots and the Limitless market snapshot from workspace memory,
/// score LONG vs SHORT using a 10-condition multi-timeframe model, apply hard
/// overrides, and write YES/NO recommendations to limitless/btc-15m/signal.
/// No external API calls — all data comes from memory.
pub struct LimitlessComputeSignalTool {
    workspace: Arc<Workspace>,
}

impl LimitlessComputeSignalTool {
    pub fn new(workspace: Arc<Workspace>) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl Tool for LimitlessComputeSignalTool {
    fn name(&self) -> &str {
        "limitless_compute_signal"
    }

    fn description(&self) -> &str {
        "Read BTC TA snapshots from ta/btc/* and market data from \
         limitless/btc-15m/snapshot. Score LONG vs SHORT with a 10-condition \
         multi-timeframe model. Apply hard overrides (RSI extremes, BB squeeze, \
         low volume). Write YES/NO recommendations to limitless/btc-15m/signal."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    async fn execute(
        &self,
        _params: serde_json::Value,
        _ctx: &JobContext,
    ) -> Result<ToolOutput, ToolError> {
        let start = std::time::Instant::now();
        let now = Utc::now();

        let ta_4h = read_ta_snapshot(&self.workspace, "4h").await?;
        let ta_1h = read_ta_snapshot(&self.workspace, "1h").await?;
        let ta_15m = read_ta_snapshot(&self.workspace, "15m").await?;
        let ta_5m = read_ta_snapshot(&self.workspace, "5m").await?;

        for ta in [&ta_4h, &ta_1h, &ta_15m, &ta_5m] {
            validate_ta_freshness(ta, 30)?;
        }

        let snapshot_doc = match self.workspace.read("limitless/btc-15m/snapshot").await {
            Ok(doc) => doc,
            Err(WorkspaceError::DocumentNotFound { .. }) => {
                return Ok(ToolOutput::success(
                    serde_json::json!({
                        "status": "skipped",
                        "reason": "No market snapshot in memory yet — limitless_fetch_markets has not run this cycle.",
                    }),
                    start.elapsed(),
                ));
            }
            Err(e) => {
                return Err(ToolError::ExecutionFailed(format!("Read snapshot failed: {e}")))
            }
        };

        let snapshot: serde_json::Value =
            serde_json::from_str(&snapshot_doc.content).map_err(|e| {
                ToolError::ExecutionFailed(format!("Invalid market snapshot JSON: {e}"))
            })?;

        // Skip entire cycle if no active markets were found upstream
        if snapshot["active"].as_bool() == Some(false)
            || snapshot["markets"]
                .as_array()
                .map_or(true, |a| a.is_empty())
        {
            return Ok(ToolOutput::success(
                serde_json::json!({
                    "status": "skipped",
                    "reason": "No active BTC 15m markets this cycle.",
                }),
                start.elapsed(),
            ));
        }

        if let Some(ts) = snapshot["fetched_at"].as_str() {
            if let Ok(fetched_at) = chrono::DateTime::parse_from_rfc3339(ts) {
                let age = now.signed_duration_since(fetched_at.with_timezone(&Utc));
                if age.num_minutes() > 5 {
                    return Ok(ToolOutput::success(
                        serde_json::json!({
                            "status": "skipped",
                            "reason": format!(
                                "Market snapshot is {} min old (max 5) — waiting for next cycle.",
                                age.num_minutes()
                            ),
                        }),
                        start.elapsed(),
                    ));
                }
            }
        }

        let markets: Vec<MarketEntry> =
            serde_json::from_value(snapshot["markets"].clone()).map_err(|e| {
                ToolError::ExecutionFailed(format!("Invalid markets array: {e}"))
            })?;

        let (long_score, short_score) = score_direction(&ta_4h, &ta_1h, &ta_15m, &ta_5m);

        // net_score > 0 = bullish, < 0 = bearish (-10 to +10)
        let net_score: i8 = long_score as i8 - short_score as i8;

        let bb_squeeze = ta_15m.bb_width < 0.015;
        let low_volume = ta_15m.volume_ratio < 0.4;

        let current_price = ta_15m.current_price;

        let market_signals: Vec<serde_json::Value> = markets
            .iter()
            .map(|m| {
                let strike = parse_strike_from_title(&m.title).unwrap_or(0.0);
                let gap_pct = if strike > 0.0 {
                    (current_price - strike) / strike * 100.0
                } else {
                    0.0
                };

                let (raw_decision, raw_reason) = if bb_squeeze || low_volume {
                    // Uncertain market conditions: skip gap logic, use raw TA scores.
                    // Tie defaults to NO (conservative).
                    let cond = if bb_squeeze { "BB squeeze" } else { "low volume" };
                    if long_score > short_score {
                        ("YES", format!("{cond}: bull={long_score} > bear={short_score} gap={gap_pct:+.2}%"))
                    } else if short_score > long_score {
                        ("NO", format!("{cond}: bear={short_score} > bull={long_score} gap={gap_pct:+.2}%"))
                    } else {
                        ("NO", format!("{cond}: scores tied ({long_score}={short_score}), default NO gap={gap_pct:+.2}%"))
                    }
                } else {
                    // Normal decision: gap position + TA momentum.
                    // Exactly at strike with equal scores → NO (conservative).
                    let d = if gap_pct > 1.0 {
                        if net_score <= -4 { "NO" } else { "YES" }
                    } else if gap_pct < -1.0 {
                        if net_score >= 4 { "YES" } else { "NO" }
                    } else if gap_pct > 0.0 {
                        if net_score >= -2 { "YES" } else { "NO" }
                    } else if gap_pct < 0.0 {
                        if net_score >= 2 { "YES" } else { "NO" }
                    } else {
                        // Exactly at strike: stronger TA side wins; tie → NO.
                        if long_score > short_score { "YES" } else { "NO" }
                    };
                    (d, format!("gap={gap_pct:+.2}% net_score={net_score:+} (bull={long_score} bear={short_score})"))
                };

                // Final override: weak YES conviction → force NO.
                // yes_score < 5 AND (yes_score - no_score) <= 1 means bullish case
                // is too weak — default to NO regardless of gap decision.
                let (decision, reason) =
                    if long_score < 5 && (long_score as i8 - short_score as i8) <= 1 {
                        (
                            "NO",
                            format!(
                                "weak YES override: yes={long_score} no={short_score} margin={} (was {raw_decision})",
                                long_score as i8 - short_score as i8,
                            ),
                        )
                    } else {
                        (raw_decision, raw_reason)
                    };

                serde_json::json!({
                    "market_id": m.market_id,
                    "title": m.title,
                    "slug": m.slug,
                    "strike": strike,
                    "current_price": current_price,
                    "gap_pct": gap_pct,
                    "decision": decision,
                    "reason": reason,
                    "yes_price": m.yes_price,
                    "no_price": m.no_price,
                    "liquidity": m.liquidity,
                })
            })
            .collect();

        let signal = serde_json::json!({
            "computed_at": now.to_rfc3339(),
            "btc_price_15m": current_price,
            "score": net_score,
            "yes_score": long_score,
            "no_score": short_score,
            "markets": market_signals,
        });

        let json = serde_json::to_string(&signal)
            .map_err(|e| ToolError::ExecutionFailed(format!("Serialize error: {e}")))?;
        self.workspace
            .write("limitless/btc-15m/signal", &json)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Memory write failed: {e}")))?;

        Ok(ToolOutput::success(signal, start.elapsed()))
    }
}

// ── Tool 4: limitless_place_orders ────────────────────────────────────────────

/// Read the signal from memory, fetch live USDC balance via BaseScan,
/// and place orders via `limitless`. Order size is 10% of available balance
/// (3% when YES/NO conviction diff ≤ 2).
///
/// Requires env vars: `BASESCAN_API_KEY`, `LIMITLESS_WALLET_ADDRESS`.
/// Order placement credentials are read from `limitless`'s own config —
/// do NOT store them in IronClaw's `.env`.
///
/// Requires: `limitless` on PATH
pub struct LimitlessPlaceOrdersTool {
    workspace: Arc<Workspace>,
}

impl LimitlessPlaceOrdersTool {
    pub fn new(workspace: Arc<Workspace>) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl Tool for LimitlessPlaceOrdersTool {
    fn name(&self) -> &str {
        "limitless_place_orders"
    }

    fn description(&self) -> &str {
        "Read the YES/NO signal from limitless/btc-15m/signal, fetch live USDC \
         balance via BaseScan (on-chain), and place orders via limitless. \
         Order size is 10% of balance (3% when conviction diff ≤ 2). Aborts if \
         signal is stale (> 10 min), score < 5, or insufficient funds. Requires \
         BASESCAN_API_KEY and LIMITLESS_WALLET_ADDRESS env vars, and limitless on PATH."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "dry_run": {
                    "type": "boolean",
                    "description": "If true, log what would be ordered but do not execute.",
                    "default": false
                }
            }
        })
    }

    fn requires_approval(&self, _params: &serde_json::Value) -> ApprovalRequirement {
        ApprovalRequirement::Always
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &JobContext,
    ) -> Result<ToolOutput, ToolError> {
        let start = std::time::Instant::now();
        let dry_run = params
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let now = Utc::now();

        // Read and validate signal — skip gracefully if not yet written
        let signal_doc = match self.workspace.read("limitless/btc-15m/signal").await {
            Ok(doc) => doc,
            Err(WorkspaceError::DocumentNotFound { .. }) => {
                return Ok(ToolOutput::success(
                    serde_json::json!({
                        "status": "skipped",
                        "reason": "No signal in memory yet — upstream steps have not run this cycle.",
                    }),
                    start.elapsed(),
                ));
            }
            Err(e) => return Err(ToolError::ExecutionFailed(format!("Read signal failed: {e}"))),
        };

        let signal: serde_json::Value =
            serde_json::from_str(&signal_doc.content).map_err(|e| {
                ToolError::ExecutionFailed(format!("Invalid signal JSON: {e}"))
            })?;

        // Skip if signal was written with no active markets
        if signal["markets"]
            .as_array()
            .map_or(false, |a| a.is_empty())
        {
            return Ok(ToolOutput::success(
                serde_json::json!({
                    "status": "skipped",
                    "reason": "No active markets in signal — nothing to order this cycle.",
                }),
                start.elapsed(),
            ));
        }

        if let Some(ts) = signal["fetched_at"].as_str() {
            if let Ok(fetched_at) = chrono::DateTime::parse_from_rfc3339(ts) {
                let age = now.signed_duration_since(fetched_at.with_timezone(&Utc));
                if age.num_minutes() > 10 {
                    return Ok(ToolOutput::success(
                        serde_json::json!({
                            "status": "skipped",
                            "reason": format!(
                                "Signal is {} min old (max 10) — waiting for next cycle.",
                                age.num_minutes()
                            ),
                        }),
                        start.elapsed(),
                    ));
                }
            }
        }

        let score = signal["score"].as_i64().unwrap_or(0);

        // Load snapshot to get per-market slug, prices, and liquidity.
        // The LLM signal only has market_id + decision; metadata lives in snapshot.
        let snapshot_doc = self.workspace.read("limitless/btc-15m/snapshot").await.ok();
        let snapshot_markets: std::collections::HashMap<String, serde_json::Value> =
            snapshot_doc
                .as_ref()
                .and_then(|doc| serde_json::from_str::<serde_json::Value>(&doc.content).ok())
                .and_then(|v| v["markets"].as_array().cloned())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|m| {
                    let id = m["market_id"].as_str()?.to_string();
                    Some((id, m))
                })
                .collect();

        // Filter to markets where the LLM decided YES or NO (not SKIP).
        let actionable: Vec<serde_json::Value> = signal["markets"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|m| {
                let dec = m["decision"].as_str().unwrap_or("");
                dec == "YES" || dec == "NO"
            })
            .collect();

        if actionable.is_empty() {
            return Ok(ToolOutput::success(
                serde_json::json!({
                    "status": "skipped",
                    "reason": "No actionable YES/NO markets after filtering",
                }),
                start.elapsed(),
            ));
        }

        // Large-order pre-approval gate (> $50 total)
        let order_approval = self
            .workspace
            .read("limitless/order-approval")
            .await
            .ok();

        // Read credentials once — needed for every order call.
        let api_key = read_env_var("LIMITLESS_API_KEY").ok_or_else(|| {
            ToolError::ExecutionFailed(
                "LIMITLESS_API_KEY not set — add to ~/.ironclaw/.env".into(),
            )
        })?;
        // LIMITLESS_PRIVATE_KEY is injected into the CLI child process env at call-time.
        if read_env_var("LIMITLESS_PRIVATE_KEY").is_none() {
            return Err(ToolError::ExecutionFailed(
                "LIMITLESS_PRIVATE_KEY not set — add to ~/.ironclaw/.env".into(),
            ));
        }

        let balance_result = fetch_usdc_balance_via_basescan().await;

        // Use yes_score/no_score diff if present; fall back to |score|.
        let yes_score = signal["yes_score"].as_i64().unwrap_or(score.max(0));
        let no_score = signal["no_score"].as_i64().unwrap_or((-score).max(0));
        let conviction_diff = (yes_score - no_score).abs();
        let weak_conviction = conviction_diff <= 2;

        // Store raw balance for per-market low-liquidity 3% override.
        let (balance_usdc, order_size, balance_source) = match balance_result {
            Ok(bal) if bal > 0.0 => {
                let size_pct = if weak_conviction { 0.03 } else { 0.10 };
                let sz = (bal * size_pct * 100.0).floor() / 100.0;
                (Some(bal), sz, format!("BaseScan (${bal:.2})"))
            }
            Ok(_) => {
                return Err(ToolError::ExecutionFailed(
                    "On-chain USDC balance is zero. Fund your account before placing orders."
                        .to_string(),
                ));
            }
            Err(e) => {
                // Fallback fixed amounts: 10% tier = $6, 3% tier = $2
                let sz = if weak_conviction { 2.0_f64 } else { 6.0_f64 };
                tracing::warn!("BaseScan balance fetch failed ({e}), using fallback ${sz:.2}");
                (None, sz, format!("fallback (BaseScan unavailable: {e})"))
            }
        };

        if order_size < 1.0 {
            return Err(ToolError::ExecutionFailed(format!(
                "Order size ${order_size:.2} < $1.00 minimum. Balance source: {balance_source}"
            )));
        }

        let total_exposure = order_size * actionable.len() as f64;
        if total_exposure > 50.0 && order_approval.is_none() {
            return Ok(ToolOutput::success(
                serde_json::json!({
                    "status": "blocked",
                    "reason": format!(
                        "Total exposure ${total_exposure:.2} exceeds $50. \
                         Ask IronClaw 'Approve Limitless orders above $50' to pre-authorize."
                    ),
                }),
                start.elapsed(),
            ));
        }

        let mut order_results: Vec<serde_json::Value> = Vec::new();

        for market in &actionable {
            let market_id = market["market_id"].as_str().unwrap_or("");
            let decision = market["decision"].as_str().unwrap_or("SKIP");
            let outcome = decision.to_lowercase();

            // Look up metadata from snapshot (slug, prices, liquidity)
            let meta = snapshot_markets.get(market_id);
            let slug = meta
                .and_then(|m| m["slug"].as_str())
                .unwrap_or(market_id);
            let liquidity = meta
                .and_then(|m| m["liquidity"].as_f64())
                .unwrap_or(0.0);

            let price = if decision == "YES" {
                meta.and_then(|m| m["yes_price"].as_f64()).unwrap_or(0.5)
            } else {
                meta.and_then(|m| m["no_price"].as_f64()).unwrap_or(0.5)
            };
            // Liquidity ≤ $5: conservative GTC + 3% size (limit order, fills when liquidity appears).
            // Liquidity > $5: score ≥ 7 → FOK (fill-or-kill); < 7 → GTC.
            let (order_type, this_order_size) = if liquidity <= 5.0 {
                let sz = balance_usdc
                    .map(|b| (b * 0.03 * 100.0).floor() / 100.0)
                    .unwrap_or(2.0);
                ("GTC", sz)
            } else if score.unsigned_abs() >= 7 {
                ("FOK", order_size)
            } else {
                ("GTC", order_size)
            };

            if dry_run {
                order_results.push(serde_json::json!({
                    "slug": slug,
                    "outcome": outcome,
                    "price": price,
                    "size_usdc": this_order_size,
                    "order_type": order_type,
                    "status": "dry_run",
                }));
                continue;
            }

            let result = execute_limitless_order(
                &api_key,
                slug,
                &outcome,
                price,
                this_order_size,
                order_type,
            )
            .await
            .map(|stdout| serde_json::from_str(&stdout).unwrap_or(serde_json::json!({"raw": stdout})));

            order_results.push(match result {
                Ok(v) => v,
                Err(e) => serde_json::json!({
                    "slug": slug,
                    "outcome": outcome,
                    "price": price,
                    "size_usdc": this_order_size,
                    "order_type": order_type,
                    "status": "error",
                    "error": e.to_string(),
                }),
            });
        }

        let result = serde_json::json!({
            "executed_at": now.to_rfc3339(),
            "signal_score": score,
            "yes_score": signal["yes_score"],
            "no_score": signal["no_score"],
            "balance_source": balance_source,
            "order_size_usdc": order_size,
            "dry_run": dry_run,
            "orders": order_results,
        });

        let json = serde_json::to_string(&result)
            .map_err(|e| ToolError::ExecutionFailed(format!("Serialize error: {e}")))?;
        self.workspace
            .write("limitless/btc-15m/orders/latest", &json)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Memory write failed: {e}")))?;

        Ok(ToolOutput::success(result, start.elapsed()))
    }
}

// ── Tool 5: limitless_check_balance ───────────────────────────────────────────

/// Place a single order on Limitless Exchange via direct HTTP (no CLI).
///
/// Reads LIMITLESS_API_KEY, LIMITLESS_PRIVATE_KEY from process env or ~/.ironclaw/.env.
/// Signs the order with EIP-712 (alloy) and POSTs to https://api.limitless.exchange/orders.
pub struct LimitlessPlaceOrderHttpTool;

impl LimitlessPlaceOrderHttpTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for LimitlessPlaceOrderHttpTool {
    fn name(&self) -> &str {
        "limitless_place_order_http"
    }

    fn description(&self) -> &str {
        "Place a single order on Limitless Exchange via direct HTTP API (no CLI required). \
         Signs EIP-712 order with private key and POSTs to the Limitless API. \
         Parameters: slug (market slug), outcome (yes/no), size (USDC to spend), \
         order_type (FOK or GTC, default FOK), price (required for GTC, 0.01–0.99). \
         Reads LIMITLESS_API_KEY and LIMITLESS_PRIVATE_KEY from env or ~/.ironclaw/.env."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "slug":       { "type": "string", "description": "Market slug" },
                "outcome":    { "type": "string", "enum": ["yes", "no"] },
                "size":       { "type": "number", "description": "USDC to spend (FOK) or shares (GTC)" },
                "order_type": { "type": "string", "enum": ["FOK", "GTC"], "default": "FOK" },
                "price":      { "type": "number", "description": "Price 0.01–0.99, required for GTC" }
            },
            "required": ["slug", "outcome", "size"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &JobContext,
    ) -> Result<ToolOutput, ToolError> {
        let start = std::time::Instant::now();

        let slug = params["slug"].as_str()
            .ok_or_else(|| ToolError::InvalidParameters("slug required".into()))?;
        let outcome = params["outcome"].as_str()
            .ok_or_else(|| ToolError::InvalidParameters("outcome required (yes/no)".into()))?
            .to_lowercase();
        let size = params["size"].as_f64()
            .ok_or_else(|| ToolError::InvalidParameters("size required".into()))?;
        let order_type = params["order_type"].as_str().unwrap_or("FOK").to_uppercase();
        let price_opt = params["price"].as_f64();

        if order_type == "GTC" && price_opt.is_none() {
            return Err(ToolError::InvalidParameters("price required for GTC orders".into()));
        }

        let api_key = read_env_var("LIMITLESS_API_KEY")
            .ok_or_else(|| ToolError::ExecutionFailed("LIMITLESS_API_KEY not set — add to ~/.ironclaw/.env".into()))?;
        let pk_str = read_env_var("LIMITLESS_PRIVATE_KEY")
            .ok_or_else(|| ToolError::ExecutionFailed("LIMITLESS_PRIVATE_KEY not set — add to ~/.ironclaw/.env".into()))?;

        let resp = limitless_http_place_order(
            &api_key, &pk_str, slug, &outcome, size, &order_type, price_opt,
        ).await?;

        Ok(ToolOutput::success(resp, start.elapsed()))
    }
}

/// Full EIP-712 signed order via direct HTTP — no CLI dependency.
async fn limitless_http_place_order(
    api_key: &str,
    pk_hex: &str,
    slug: &str,
    outcome: &str, // "yes" or "no"
    size: f64,
    order_type: &str, // "FOK" or "GTC"
    price: Option<f64>,
) -> Result<serde_json::Value, ToolError> {
    use alloy::primitives::{Address, U256};
    use alloy::signers::local::PrivateKeySigner;
    use alloy::signers::Signer;
    use alloy::sol;
    use alloy::sol_types::SolStruct;

    const BASE_URL: &str = "https://api.limitless.exchange";
    const CHAIN_ID: u64 = 8453; // Base

    sol! {
        #[derive(Debug)]
        struct Order {
            uint256 salt;
            address maker;
            address signer;
            address taker;
            uint256 tokenId;
            uint256 makerAmount;
            uint256 takerAmount;
            uint256 expiration;
            uint256 nonce;
            uint256 feeRateBps;
            uint8 side;
            uint8 signatureType;
        }
    }

    let err = |s: String| ToolError::ExecutionFailed(s);

    // Build HTTP client with API key header
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "X-API-Key",
        reqwest::header::HeaderValue::from_str(api_key)
            .map_err(|e| err(format!("Invalid API key: {e}")))?,
    );
    let http = reqwest::Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| err(format!("HTTP client: {e}")))?;

    // Derive signer and maker address from private key
    let pk_bytes = hex::decode(pk_hex.trim_start_matches("0x"))
        .map_err(|e| err(format!("Invalid private key hex: {e}")))?;
    let signer = PrivateKeySigner::from_slice(&pk_bytes)
        .map_err(|e| err(format!("Invalid private key: {e}")))?;
    let maker: Address = signer.address();
    let maker_hex = format!("{maker:#x}");

    // Fetch profile → ownerId + feeRateBps
    let profile: serde_json::Value = http
        .get(format!("{BASE_URL}/profiles/public/{maker_hex}"))
        .send().await.map_err(|e| err(format!("Profile request failed: {e}")))?
        .json().await.map_err(|e| err(format!("Profile parse failed: {e}")))?;
    let owner_id = profile["id"].as_u64()
        .ok_or_else(|| err(format!("Profile missing id: {profile}")))?;
    let fee_rate_bps: u64 = profile["rank"]["feeRateBps"].as_u64().unwrap_or(300);

    // Fetch market → venue.exchange + tokens.yes/no
    let market: serde_json::Value = http
        .get(format!("{BASE_URL}/markets/{slug}"))
        .send().await.map_err(|e| err(format!("Market request failed: {e}")))?
        .json().await.map_err(|e| err(format!("Market parse failed: {e}")))?;
    let exchange_str = market["venue"]["exchange"].as_str()
        .ok_or_else(|| err(format!("Market missing venue.exchange: {market}")))?;
    let token_id_str = if outcome == "yes" {
        market["tokens"]["yes"].as_str()
    } else {
        market["tokens"]["no"].as_str()
    }.ok_or_else(|| err(format!("Market missing token id for outcome={outcome}")))?;

    let venue_exchange: Address = exchange_str.parse()
        .map_err(|e| err(format!("Invalid exchange address {exchange_str}: {e}")))?;
    let token_id = U256::from_str_radix(token_id_str, 10)
        .map_err(|e| err(format!("Invalid tokenId {token_id_str}: {e}")))?;
    let zero_addr: Address = "0x0000000000000000000000000000000000000000".parse().unwrap();

    // Generate random salt (JS-safe integer range)
    let salt_val = {
        let mut buf = [0u8; 8];
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut buf);
        let v = u64::from_be_bytes(buf) & ((1u64 << 53) - 1);
        U256::from(v)
    };

    // Build order struct
    let scale = 1_000_000u64; // 1e6 micro-USDC
    let (maker_amount, taker_amount, order_price_field) = if order_type == "FOK" {
        let ma = U256::from((size * scale as f64) as u128);
        (ma, U256::from(1u64), None)
    } else {
        let p = price.unwrap();
        let shares = size;
        let usdc = p * shares;
        let ma = U256::from((usdc * scale as f64) as u128);
        let ta = U256::from((shares * scale as f64) as u128);
        (ma, ta, Some(p))
    };

    let order = Order {
        salt: salt_val,
        maker,
        signer: maker,
        taker: zero_addr,
        tokenId: token_id,
        makerAmount: maker_amount,
        takerAmount: taker_amount,
        expiration: U256::ZERO,
        nonce: U256::ZERO,
        feeRateBps: U256::from(fee_rate_bps),
        side: 0u8, // buy
        signatureType: 0u8,
    };

    // EIP-712 sign
    let domain = alloy::sol_types::Eip712Domain {
        name: Some("Limitless CTF Exchange".into()),
        version: Some("1".into()),
        chain_id: Some(U256::from(CHAIN_ID)),
        verifying_contract: Some(venue_exchange),
        salt: None,
    };
    let signing_hash = order.eip712_signing_hash(&domain);
    let signature = signer.sign_hash(&signing_hash).await
        .map_err(|e| err(format!("Signing failed: {e}")))?;

    let mut sig_bytes = Vec::with_capacity(65);
    sig_bytes.extend_from_slice(&signature.r().to_be_bytes::<32>());
    sig_bytes.extend_from_slice(&signature.s().to_be_bytes::<32>());
    sig_bytes.push(if signature.v() { 28 } else { 27 });
    let sig_hex = format!("0x{}", hex::encode(&sig_bytes));

    // Build order payload
    let mut order_payload = serde_json::json!({
        "salt":          salt_val.to::<u64>(),
        "maker":         format!("{maker:#x}"),
        "signer":        format!("{maker:#x}"),
        "taker":         "0x0000000000000000000000000000000000000000",
        "tokenId":       token_id_str,
        "makerAmount":   maker_amount.to::<u64>(),
        "takerAmount":   taker_amount.to::<u64>(),
        "expiration":    "0",
        "nonce":         0u64,
        "feeRateBps":    fee_rate_bps,
        "side":          0u64,
        "signatureType": 0u64,
        "signature":     sig_hex,
    });
    if let Some(p) = order_price_field {
        order_payload.as_object_mut().unwrap().insert("price".into(), serde_json::json!(p));
    }

    let payload = serde_json::json!({
        "order":      order_payload,
        "orderType":  order_type,
        "marketSlug": slug,
        "ownerId":    owner_id,
    });

    let resp = http
        .post(format!("{BASE_URL}/orders"))
        .json(&payload)
        .send().await.map_err(|e| err(format!("Order submit failed: {e}")))?;

    let status = resp.status();
    let body: serde_json::Value = resp.json().await
        .unwrap_or_else(|_| serde_json::json!({ "raw": "non-JSON response" }));

    if !status.is_success() {
        return Err(err(format!("Order rejected (HTTP {status}): {body}")));
    }

    Ok(serde_json::json!({
        "status": "submitted",
        "slug": slug,
        "outcome": outcome,
        "size_usdc": size,
        "order_type": order_type,
        "response": body,
    }))
}

/// Check USDC balance and open positions via `limitless-cli`.
/// Credentials are read from limitless-cli's own config — nothing stored in IronClaw .env.
pub struct LimitlessCheckBalanceTool;

impl LimitlessCheckBalanceTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for LimitlessCheckBalanceTool {
    fn name(&self) -> &str {
        "limitless_check_balance"
    }

    fn description(&self) -> &str {
        "Check your USDC trading allowance and open positions on Limitless Exchange via \
         limitless-cli. Returns available balance (from portfolio allowance) and current \
         open positions. Credentials are read from limitless-cli's own config."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    async fn execute(
        &self,
        _params: serde_json::Value,
        _ctx: &JobContext,
    ) -> Result<ToolOutput, ToolError> {
        let start = std::time::Instant::now();

        // Run both commands concurrently
        let (allowance_result, positions_result) = tokio::join!(
            run_limitless_cli_json(&["portfolio", "allowance"]),
            run_limitless_cli_json(&["portfolio", "positions"]),
        );

        let allowance = allowance_result.unwrap_or_else(|e| serde_json::json!({ "error": e }));
        let positions = positions_result.unwrap_or_else(|e| serde_json::json!({ "error": e }));

        Ok(ToolOutput::success(
            serde_json::json!({
                "allowance": allowance,
                "positions": positions,
            }),
            start.elapsed(),
        ))
    }
}

/// Fetch on-chain USDC balance from BaseScan for the configured wallet.
pub struct LimitlessBaseScanBalanceTool;

impl LimitlessBaseScanBalanceTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for LimitlessBaseScanBalanceTool {
    fn name(&self) -> &str {
        "limitless_basescan_balance"
    }

    fn description(&self) -> &str {
        "Fetch on-chain USDC balance from BaseScan for the wallet set in \
         LIMITLESS_WALLET_ADDRESS. Reads BASESCAN_API_KEY and LIMITLESS_WALLET_ADDRESS \
         from process env or ~/.ironclaw/.env."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    async fn execute(
        &self,
        _params: serde_json::Value,
        _ctx: &JobContext,
    ) -> Result<ToolOutput, ToolError> {
        let start = std::time::Instant::now();
        let wallet = std::env::var("LIMITLESS_WALLET_ADDRESS").unwrap_or_else(|_| "unknown".into());
        let balance = fetch_usdc_balance_via_basescan().await?;
        Ok(ToolOutput::success(
            serde_json::json!({
                "wallet": wallet,
                "usdc_balance": balance,
                "source": "BaseScan (on-chain)",
            }),
            start.elapsed(),
        ))
    }
}

/// Run `limitless <args> -o json` and parse stdout as JSON.
async fn run_limitless_cli_json(args: &[&str]) -> Result<serde_json::Value, String> {
    let mut cmd = limitless_cli_cmd();
    cmd.args(args);
    cmd.args(["-o", "json"]);
    if let Ok(k) = std::env::var("LIMITLESS_API_KEY") {
        cmd.env("LIMITLESS_API_KEY", k);
    }
    if let Ok(k) = std::env::var("LIMITLESS_PRIVATE_KEY") {
        cmd.env("LIMITLESS_PRIVATE_KEY", k);
    }
    cmd.kill_on_drop(true);

    let output = tokio::time::timeout(Duration::from_secs(30), cmd.output())
        .await
        .map_err(|_| "limitless timed out".to_string())?
        .map_err(|e| format!("limitless failed to start: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        return Err(format!(
            "exit {}: {stdout}{stderr}",
            output.status.code().unwrap_or(-1)
        ));
    }

    serde_json::from_str(stdout.trim()).map_err(|e| format!("JSON parse failed: {e}: {stdout}"))
}

// ── Private helpers ───────────────────────────────────────────────────────────

fn build_http_client() -> Result<reqwest::Client, ToolError> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("ironclaw/1.0")
        .build()
        .map_err(|e| ToolError::ExecutionFailed(format!("HTTP client error: {e}")))
}

async fn fetch_binance_klines(
    client: &reqwest::Client,
    interval: &str,
) -> Result<Vec<[f64; 8]>, String> {
    let url = format!(
        "https://api.binance.com/api/v3/klines?symbol=BTCUSDT&interval={interval}&limit=100"
    );
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let raw: Vec<serde_json::Value> = resp
        .json()
        .await
        .map_err(|e| format!("Parse error: {e}"))?;

    raw.iter()
        .map(|row| {
            let arr = row.as_array().ok_or("Not an array")?;
            let parse = |idx: usize| -> Result<f64, &'static str> {
                match arr.get(idx) {
                    Some(v) => v
                        .as_f64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                        .ok_or("Not a number"),
                    None => Err("Missing field"),
                }
            };
            Ok([
                parse(1)?, // open
                parse(2)?, // high
                parse(3)?, // low
                parse(4)?, // close
                parse(5)?, // volume (BTC)
                parse(7)?, // quote volume (USDT)
                arr.get(0)
                    .and_then(|v| v.as_f64())
                    .ok_or("Missing openTime")?,
                arr.get(6)
                    .and_then(|v| v.as_f64())
                    .ok_or("Missing closeTime")?,
            ])
        })
        .collect::<Result<Vec<_>, &str>>()
        .map_err(|e| e.to_string())
}

fn compute_ta(
    klines: &[[f64; 8]],
    interval: &str,
    fetched_at: &str,
) -> Result<TaSnapshot, String> {
    if klines.len() < 52 {
        return Err(format!(
            "Only {} candles, need ≥ 52 (50 + 2 buffer)",
            klines.len()
        ));
    }

    // Use confirmed candle: exclude the last (currently forming) candle
    let confirmed = &klines[..klines.len() - 1];
    let len = confirmed.len();

    let closes: Vec<f64> = confirmed.iter().map(|k| k[3]).collect();
    let highs: Vec<f64> = confirmed.iter().map(|k| k[1]).collect();
    let lows: Vec<f64> = confirmed.iter().map(|k| k[2]).collect();
    let opens: Vec<f64> = confirmed.iter().map(|k| k[0]).collect();
    let volumes: Vec<f64> = confirmed.iter().map(|k| k[4]).collect();

    let current_price = *closes.last().unwrap();
    let open_price = *opens.last().unwrap();
    let high_20 = highs[len - 20..].iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let low_20 = lows[len - 20..].iter().cloned().fold(f64::INFINITY, f64::min);
    let change_pct = (current_price - closes[0]) / closes[0] * 100.0;

    let sma_20 = sma(&closes, 20).ok_or("SMA20 failed")?;
    let sma_50 = sma(&closes, 50).ok_or("SMA50 failed")?;

    let ema12_series = ema_series(&closes, 12);
    let ema26_series = ema_series(&closes, 26);
    let ema_12 = *ema12_series.last().ok_or("EMA12 series empty")?;
    let ema_26 = *ema26_series.last().ok_or("EMA26 series empty")?;

    let (macd_line, signal_line, histogram, prev_histogram) =
        macd_full(&closes).ok_or("MACD failed")?;

    let rsi = rsi_14(&closes).ok_or("RSI failed")?;

    let (bb_upper, bb_middle, bb_lower, bb_width, bb_position) =
        bollinger(&closes, 20).ok_or("Bollinger Bands failed")?;

    let volume_avg_20 = sma(&volumes, 20).ok_or("Volume SMA failed")?;
    let volume_current = *volumes.last().unwrap();
    let volume_ratio = if volume_avg_20 > 0.0 {
        volume_current / volume_avg_20
    } else {
        1.0
    };

    Ok(TaSnapshot {
        interval: interval.to_string(),
        fetched_at: fetched_at.to_string(),
        current_price,
        open_price,
        high_20,
        low_20,
        change_pct,
        sma_20,
        sma_50,
        ema_12,
        ema_26,
        macd_line,
        signal_line,
        histogram,
        prev_histogram,
        rsi,
        bb_upper,
        bb_middle,
        bb_lower,
        bb_width,
        bb_position,
        volume_avg_20,
        volume_current,
        volume_ratio,
    })
}

/// Returns (parsed_markets, raw_items) so the caller can expose raw shape for debugging.
async fn fetch_limitless_markets_15m(
    client: &reqwest::Client,
) -> Result<(Vec<MarketEntry>, Vec<serde_json::Value>), ToolError> {
    let resp = client
        .get("https://api.limitless.exchange/markets/active/2")
        .send()
        .await
        .map_err(|e| ToolError::ExecutionFailed(format!("Limitless request failed: {e}")))?;

    if !resp.status().is_success() {
        return Err(ToolError::ExecutionFailed(format!(
            "Limitless API returned HTTP {}",
            resp.status()
        )));
    }

    let raw: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| ToolError::ExecutionFailed(format!("Limitless parse error: {e}")))?;

    // The Limitless Exchange API may return:
    //   - a top-level array
    //   - { "markets": [...] }
    //   - { "data": [...] }
    //   - { "data": { "markets": [...] } }
    let all_markets: Vec<serde_json::Value> = if let Some(arr) = raw.as_array() {
        arr.clone()
    } else if let Some(arr) = raw.get("markets").and_then(|v| v.as_array()) {
        arr.clone()
    } else if let Some(arr) = raw.get("data").and_then(|v| v.as_array()) {
        arr.clone()
    } else if let Some(arr) = raw
        .get("data")
        .and_then(|d| d.get("markets"))
        .and_then(|v| v.as_array())
    {
        arr.clone()
    } else {
        Vec::new()
    };

    let markets: Vec<MarketEntry> = all_markets
        .iter()
        .filter(|m| is_btc_15m_open_market(m))
        .filter_map(|m| parse_market_entry(m))
        .collect();

    Ok((markets, all_markets))
}

/// Post-fetch filter: keep only open BTC 15-minute markets.
///
/// Category=2 is "Crypto" (not BTC-only), so after fetching all active crypto
/// markets we apply three checks:
/// 1. Not resolved (still open for trading)
/// 2. Title/question contains "BTC" or "Bitcoin"
/// 3. `categories` array contains "15 min" or "15m"
fn is_btc_15m_open_market(m: &serde_json::Value) -> bool {
    // 1. Reject resolved/settled markets.
    if m["resolved"].as_bool().unwrap_or(false) {
        return false;
    }

    // 2. Must be a BTC market.
    let text = m["title"]
        .as_str()
        .or_else(|| m["question"].as_str())
        .unwrap_or("");
    let lc = text.to_ascii_lowercase();
    if !lc.contains("btc") && !lc.contains("bitcoin") {
        return false;
    }

    // 3. Must be a 15-minute market — determined by the `categories` array
    //    which contains "15 min" for 15m markets (e.g. ["Crypto", "15 min", "ypp"]).
    m["categories"]
        .as_array()
        .map(|cats| {
            cats.iter().any(|c| {
                let s = c.as_str().unwrap_or("").to_ascii_lowercase();
                s.contains("15 min") || s.contains("15m")
            })
        })
        .unwrap_or(false)
}

fn parse_market_entry(m: &serde_json::Value) -> Option<MarketEntry> {
    // Support multiple field name conventions used by Limitless Exchange API.
    // NOTE: The real API returns `id` as an integer and `conditionId` as a hex string.
    let market_id = m["conditionId"]
        .as_str()
        .map(|s| s.to_string())
        .or_else(|| m["id"].as_u64().map(|n| n.to_string()))
        .or_else(|| m["id"].as_str().map(|s| s.to_string()))
        .or_else(|| m["marketId"].as_str().map(|s| s.to_string()))
        .or_else(|| m["address"].as_str().map(|s| s.to_string()))?;

    let title = m["title"]
        .as_str()
        .or_else(|| m["question"].as_str())?
        .to_string();

    let slug = m["slug"]
        .as_str()
        .or_else(|| m["market_slug"].as_str())
        .unwrap_or(&title)
        .to_string();

    // YES price: outcomes[0].price, or prices[0], or yesPrice, or prices.yes
    let yes_price = m["outcomes"]
        .get(0)
        .and_then(|o| o["price"].as_f64())
        .or_else(|| m["prices"].get(0).and_then(|v| v.as_f64()))
        .or_else(|| m["yesPrice"].as_f64())
        .or_else(|| m["prices"]["yes"].as_f64())
        .or_else(|| m["outcomeTokenMarginalPrices"].get(0).and_then(|v| v.as_str()).and_then(|s| s.parse().ok()))
        .unwrap_or(0.5);

    // NO price: outcomes[1].price, or prices[1], or noPrice, or prices.no
    let no_price = m["outcomes"]
        .get(1)
        .and_then(|o| o["price"].as_f64())
        .or_else(|| m["prices"].get(1).and_then(|v| v.as_f64()))
        .or_else(|| m["noPrice"].as_f64())
        .or_else(|| m["prices"]["no"].as_f64())
        .or_else(|| m["outcomeTokenMarginalPrices"].get(1).and_then(|v| v.as_str()).and_then(|s| s.parse().ok()))
        .unwrap_or(1.0 - yes_price);

    // volume is returned as a string of micro-USDC (6 decimals); convert to USDC float.
    let volume_24h = m["volume24h"]
        .as_f64()
        .or_else(|| m["volumeNum"].as_f64())
        .or_else(|| m["volume"].as_f64())
        .or_else(|| m["volume24h"].as_str().and_then(|s| s.parse::<f64>().ok()))
        .or_else(|| {
            m["volume"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .map(|v| v / 1_000_000.0)
        })
        .or_else(|| {
            m["volumeFormatted"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
        })
        .unwrap_or(0.0);

    let liquidity = m["liquidity"]
        .as_f64()
        .or_else(|| m["liquidityNum"].as_f64())
        .or_else(|| m["collateralVolume"].as_f64())
        .or_else(|| m["liquidity"].as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0.0);

    Some(MarketEntry {
        market_id,
        title,
        slug,
        yes_price,
        no_price,
        volume_24h,
        liquidity,
    })
}

async fn read_ta_snapshot(
    workspace: &Workspace,
    interval: &str,
) -> Result<TaSnapshot, ToolError> {
    let path = format!("ta/btc/{interval}");
    let doc = workspace.read(&path).await.map_err(|e| match e {
        WorkspaceError::DocumentNotFound { .. } => ToolError::ExecutionFailed(format!(
            "TA snapshot for {interval} not found. Run btc_fetch_ta first."
        )),
        other => ToolError::ExecutionFailed(format!("Read ta/btc/{interval} failed: {other}")),
    })?;
    serde_json::from_str(&doc.content).map_err(|e| {
        ToolError::ExecutionFailed(format!("Invalid TA snapshot for {interval}: {e}"))
    })
}

fn validate_ta_freshness(ta: &TaSnapshot, max_minutes: i64) -> Result<(), ToolError> {
    if let Ok(fetched_at) = chrono::DateTime::parse_from_rfc3339(&ta.fetched_at) {
        let age = Utc::now().signed_duration_since(fetched_at.with_timezone(&Utc));
        if age.num_minutes() > max_minutes {
            return Err(ToolError::ExecutionFailed(format!(
                "TA {} is {} min old (max {}). Run btc_fetch_ta.",
                ta.interval,
                age.num_minutes(),
                max_minutes
            )));
        }
    }
    Ok(())
}

/// Score LONG and SHORT independently using a 10-condition model.
/// Returns (long_score, short_score) each in range 0–10.
fn score_direction(
    ta_4h: &TaSnapshot,
    ta_1h: &TaSnapshot,
    ta_15m: &TaSnapshot,
    ta_5m: &TaSnapshot,
) -> (u8, u8) {
    let mut long: u8 = 0;
    let mut short: u8 = 0;

    // 1. RSI(14) on 1h
    if ta_1h.rsi < 40.0 {
        long += 1;
    }
    if ta_1h.rsi > 60.0 {
        short += 1;
    }

    // 2. MACD histogram on 1h trending in signal direction
    if ta_1h.histogram > ta_1h.prev_histogram {
        long += 1;
    }
    if ta_1h.histogram < ta_1h.prev_histogram {
        short += 1;
    }

    // 3. EMA(12) vs EMA(26) on 15m
    if ta_15m.ema_12 > ta_15m.ema_26 {
        long += 1;
    }
    if ta_15m.ema_12 < ta_15m.ema_26 {
        short += 1;
    }

    // 4. current_price vs SMA(50) on 4h
    if ta_4h.current_price > ta_4h.sma_50 {
        long += 1;
    }
    if ta_4h.current_price < ta_4h.sma_50 {
        short += 1;
    }

    // 5. BB position on 1h
    if ta_1h.bb_position < 0.25 {
        long += 1;
    }
    if ta_1h.bb_position > 0.75 {
        short += 1;
    }

    // 6. RSI(14) on 15m
    if ta_15m.rsi < 45.0 {
        long += 1;
    }
    if ta_15m.rsi > 55.0 {
        short += 1;
    }

    // 7. MACD histogram on 4h
    if ta_4h.histogram > 0.0 {
        long += 1;
    }
    if ta_4h.histogram < 0.0 {
        short += 1;
    }

    // 8. Volume confirmation (adds to both — direction-agnostic momentum)
    if ta_15m.volume_ratio > 1.3 {
        long += 1;
        short += 1;
    }

    // 9. RSI(14) on 4h directional
    if ta_4h.rsi < 50.0 {
        long += 1;
    }
    if ta_4h.rsi > 50.0 {
        short += 1;
    }

    // 10. 5m MACD histogram
    if ta_5m.histogram > 0.0 {
        long += 1;
    }
    if ta_5m.histogram < 0.0 {
        short += 1;
    }

    (long, short)
}

/// Parse the strike price from a Limitless market title.
/// e.g. "BTC above $97,000 in 15m" → 97000.0
fn parse_strike_from_title(title: &str) -> Option<f64> {
    title
        .split('$')
        .nth(1)
        .and_then(|after| after.split_whitespace().next())
        .map(|s| s.replace(',', ""))
        .and_then(|s| s.trim_end_matches('.').parse::<f64>().ok())
}


/// Read an environment variable from the process environment, falling back to
/// `~/.ironclaw/.env` at call-time so credentials added after startup are visible.
fn read_env_var(key: &str) -> Option<String> {
    if let Ok(v) = std::env::var(key) {
        return Some(v);
    }
    let env_path = crate::bootstrap::ironclaw_env_path();
    if env_path.exists() {
        if let Ok(iter) = dotenvy::from_path_iter(&env_path) {
            for item in iter.flatten() {
                if item.0 == key {
                    return Some(item.1);
                }
            }
        }
    }
    None
}

/// Resolve the `limitless` binary path.
///
/// Checks `LIMITLESS_CLI_PATH` first (allows pointing to a full path like
/// `C:\TrustFlow\limitless-cli\target\release\limitless.exe`), then falls
/// back to `"limitless"` (assumes it's on PATH).
fn limitless_cli_cmd() -> tokio::process::Command {
    let bin = read_env_var("LIMITLESS_CLI_PATH")
        .unwrap_or_else(|| "limitless".to_string());
    tokio::process::Command::new(bin)
}

/// Fetch USDC balance from BaseScan (Base network on-chain balance).
///
/// Reads `BASESCAN_API_KEY` and `LIMITLESS_WALLET_ADDRESS` from process env
/// (loaded at startup from `~/.ironclaw/.env` by bootstrap).
/// USDC contract on Base: 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913 (6 decimals).
async fn fetch_usdc_balance_via_basescan() -> Result<f64, ToolError> {
    let wallet = read_env_var("LIMITLESS_WALLET_ADDRESS").ok_or_else(|| {
        ToolError::ExecutionFailed(
            "LIMITLESS_WALLET_ADDRESS not set — add it to ~/.ironclaw/.env".to_string(),
        )
    })?;

    // Call balanceOf(address) on USDC contract directly via public Base RPC.
    // No API key required.
    const USDC_BASE: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
    const BASE_RPC: &str = "https://mainnet.base.org";

    // ABI-encode balanceOf(address): selector 0x70a08231 + 32-byte zero-padded address
    let addr = wallet.trim_start_matches("0x").trim_start_matches("0X");
    let data = format!("0x70a08231{:0>64}", addr);

    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "eth_call",
        "params": [{"to": USDC_BASE, "data": data}, "latest"],
        "id": 1
    });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| ToolError::ExecutionFailed(format!("HTTP client build failed: {e}")))?;

    let body: serde_json::Value = client
        .post(BASE_RPC)
        .json(&payload)
        .send()
        .await
        .map_err(|e| ToolError::ExecutionFailed(format!("Base RPC request failed: {e}")))?
        .json()
        .await
        .map_err(|e| ToolError::ExecutionFailed(format!("Base RPC JSON parse failed: {e}")))?;

    if let Some(err) = body.get("error") {
        return Err(ToolError::ExecutionFailed(format!("Base RPC error: {err}")));
    }

    // result is a hex-encoded uint256 (USDC has 6 decimals)
    let hex = body["result"]
        .as_str()
        .ok_or_else(|| ToolError::ExecutionFailed("Base RPC result missing".to_string()))?
        .trim_start_matches("0x")
        .trim_start_matches("0X");
    let micro = u128::from_str_radix(hex, 16)
        .map_err(|e| ToolError::ExecutionFailed(format!("Balance hex parse failed: {e}: {hex}")))?;
    Ok(micro as f64 / 1_000_000.0)
}


async fn execute_limitless_order(
    api_key: &str,
    slug: &str,
    outcome: &str,
    price: f64,
    size: f64,
    order_type: &str,
) -> Result<String, ToolError> {
    let mut cmd = limitless_cli_cmd();
    // --api-key must come before the subcommand
    cmd.args(["--api-key", api_key]);
    cmd.args([
        "trading", "create",
        "--slug", slug,
        "--side", "buy",
        "--outcome", outcome,
        "--size", &format!("{size:.2}"),
        "--order-type", order_type,
        "-o", "json",
    ]);

    if order_type == "GTC" {
        cmd.args(["--price", &format!("{price:.4}")]);
    }

    // Inject private key into child process env (read from .env at call-time).
    if let Some(pk) = read_env_var("LIMITLESS_PRIVATE_KEY") {
        cmd.env("LIMITLESS_PRIVATE_KEY", pk);
    }

    cmd.kill_on_drop(true);

    let output = tokio::time::timeout(Duration::from_secs(30), cmd.output())
        .await
        .map_err(|_| ToolError::ExecutionFailed("limitless timed out after 30s".to_string()))?
        .map_err(|e| ToolError::ExecutionFailed(format!("limitless failed to start: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        return Err(ToolError::ExecutionFailed(format!(
            "limitless exit {}: {stdout}{stderr}",
            output.status.code().unwrap_or(-1),
        )));
    }

    Ok(stdout)
}

// ── TA math ───────────────────────────────────────────────────────────────────

/// Simple moving average of the last `period` values.
fn sma(values: &[f64], period: usize) -> Option<f64> {
    if period == 0 || values.len() < period {
        return None;
    }
    let slice = &values[values.len() - period..];
    Some(slice.iter().sum::<f64>() / period as f64)
}

/// Build a full EMA series seeded from the SMA of the first `period` values.
/// Returns a series of length `values.len() - period + 1`.
fn ema_series(values: &[f64], period: usize) -> Vec<f64> {
    if period == 0 || values.len() < period {
        return Vec::new();
    }
    let k = 2.0 / (period as f64 + 1.0);
    let seed = values[..period].iter().sum::<f64>() / period as f64;
    let mut series = Vec::with_capacity(values.len() - period + 1);
    series.push(seed);
    for &v in &values[period..] {
        let prev = *series.last().unwrap();
        series.push(v * k + prev * (1.0 - k));
    }
    series
}

/// Compute MACD line, signal line, current histogram, and previous histogram.
/// Returns None if there are insufficient candles.
fn macd_full(closes: &[f64]) -> Option<(f64, f64, f64, f64)> {
    let fast = ema_series(closes, 12); // len = closes.len() - 11
    let slow = ema_series(closes, 26); // len = closes.len() - 25

    if slow.is_empty() || fast.is_empty() {
        return None;
    }

    // fast[i] corresponds to close index (11 + i)
    // slow[i] corresponds to close index (25 + i)
    // MACD line at slow index i: fast[14 + i] - slow[i]
    let macd_line_series: Vec<f64> = slow
        .iter()
        .enumerate()
        .map(|(i, &s)| fast[14 + i] - s)
        .collect();

    if macd_line_series.len() < 9 {
        return None;
    }

    let signal_series = ema_series(&macd_line_series, 9);
    if signal_series.is_empty() {
        return None;
    }

    let macd_line = *macd_line_series.last()?;
    let signal_line = *signal_series.last()?;
    let histogram = macd_line - signal_line;

    let prev_macd = if macd_line_series.len() >= 2 {
        macd_line_series[macd_line_series.len() - 2]
    } else {
        macd_line
    };
    let prev_signal = if signal_series.len() >= 2 {
        signal_series[signal_series.len() - 2]
    } else {
        signal_line
    };
    let prev_histogram = prev_macd - prev_signal;

    Some((macd_line, signal_line, histogram, prev_histogram))
}

/// RSI-14 using Wilder's smoothing.
fn rsi_14(closes: &[f64]) -> Option<f64> {
    if closes.len() < 15 {
        return None;
    }
    let changes: Vec<f64> = closes.windows(2).map(|w| w[1] - w[0]).collect();

    let mut avg_gain = changes[..14].iter().map(|&c| c.max(0.0)).sum::<f64>() / 14.0;
    let mut avg_loss = changes[..14].iter().map(|&c| (-c).max(0.0)).sum::<f64>() / 14.0;

    for &c in &changes[14..] {
        avg_gain = (avg_gain * 13.0 + c.max(0.0)) / 14.0;
        avg_loss = (avg_loss * 13.0 + (-c).max(0.0)) / 14.0;
    }

    if avg_loss == 0.0 {
        return Some(100.0);
    }
    Some(100.0 - 100.0 / (1.0 + avg_gain / avg_loss))
}

/// Bollinger Bands: 20-period, 2 standard deviations.
/// Returns (upper, middle, lower, width, position).
fn bollinger(closes: &[f64], period: usize) -> Option<(f64, f64, f64, f64, f64)> {
    let middle = sma(closes, period)?;
    let slice = &closes[closes.len() - period..];
    let variance = slice.iter().map(|&c| (c - middle).powi(2)).sum::<f64>() / period as f64;
    let std_dev = variance.sqrt();
    let upper = middle + 2.0 * std_dev;
    let lower = middle - 2.0 * std_dev;
    let width = if middle > 0.0 {
        (upper - lower) / middle
    } else {
        0.0
    };
    let current = *closes.last()?;
    let position = if upper > lower {
        (current - lower) / (upper - lower)
    } else {
        0.5
    };
    Some((upper, middle, lower, width, position))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sma_basic() {
        let closes = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(sma(&closes, 3), Some(4.0)); // last 3: 3,4,5 → avg 4
        assert_eq!(sma(&closes, 5), Some(3.0));
        assert_eq!(sma(&closes, 6), None); // not enough
    }

    #[test]
    fn test_ema_series_length() {
        let closes = vec![1.0; 30];
        let series = ema_series(&closes, 12);
        assert_eq!(series.len(), 30 - 12 + 1); // 19 values
    }

    #[test]
    fn test_ema_converges_to_constant() {
        // With all-same values, EMA should equal that value
        let closes = vec![100.0; 50];
        let series = ema_series(&closes, 12);
        let last = series.last().unwrap();
        assert!((last - 100.0).abs() < 1e-9, "EMA={last} expected ~100");
    }

    #[test]
    fn test_rsi_all_up() {
        // All gains → RSI = 100
        let mut closes = vec![0.0; 30];
        for i in 0..30 {
            closes[i] = i as f64;
        }
        let rsi = rsi_14(&closes).unwrap();
        assert!((rsi - 100.0).abs() < 1e-9, "RSI={rsi}");
    }

    #[test]
    fn test_rsi_all_down() {
        let mut closes = vec![0.0; 30];
        for i in 0..30 {
            closes[i] = (30 - i) as f64;
        }
        let rsi = rsi_14(&closes).unwrap();
        // All losses → avg_gain=0, RSI=0
        assert!(rsi < 1.0, "RSI={rsi} expected ~0");
    }

    #[test]
    fn test_bollinger_constant_price() {
        let closes = vec![100.0; 30];
        let (upper, middle, lower, width, position) = bollinger(&closes, 20).unwrap();
        assert!((middle - 100.0).abs() < 1e-9);
        assert!((upper - lower).abs() < 1e-9); // zero std dev
        assert!((width).abs() < 1e-9);
        assert!((position - 0.5).abs() < 1e-9); // degenerate: default 0.5
    }

    #[test]
    fn test_parse_strike_from_title() {
        assert_eq!(
            parse_strike_from_title("BTC above $97,000 in 15m"),
            Some(97000.0)
        );
        assert_eq!(
            parse_strike_from_title("BTC above $97500 in 15m"),
            Some(97500.0)
        );
        assert_eq!(parse_strike_from_title("No dollar sign"), None);
    }

    #[test]
    fn test_macd_full_constant() {
        // With constant prices, fast=slow=constant, MACD=0
        let closes = vec![50.0_f64; 100];
        let (macd_line, signal_line, histogram, _prev) = macd_full(&closes).unwrap();
        assert!(macd_line.abs() < 1e-9, "macd_line={macd_line}");
        assert!(signal_line.abs() < 1e-9, "signal_line={signal_line}");
        assert!(histogram.abs() < 1e-9, "histogram={histogram}");
    }

    #[test]
    fn test_compute_ta_needs_minimum_candles() {
        let klines = vec![[0.0_f64; 8]; 30]; // too few
        assert!(compute_ta(&klines, "15m", "2026-01-01T00:00:00Z").is_err());
    }
}
