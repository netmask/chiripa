//! chiripa.mx — Melate numbers by pure statistics.
//!
//! Serves instantly at boot from the seed CSV embedded in the binary; a
//! background task fetches the official CSV right away and every 48 hours,
//! backs it up to a Tigris bucket, and rebuilds the page and stats. If the
//! official source is down and there is no data yet, the Tigris backup is
//! used. All the math lives in `stats.rs`; the browser only formats it.

mod stats;

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Json};
use axum::routing::get;
use axum::Router;
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};

use stats::{extract_pot, generate, next_draw, parse, plan, Draw, Stats, STRATEGIES, WINDOW};

const CSV_URL: &str = "https://pronosticos.gob.mx/Documentos/Historicos/Melate.csv";
const POT_URL: &str = "https://www.loterianacional.gob.mx/";
const SEED: &str = include_str!("../assets/Melate.csv");
const TEMPLATE: &str = include_str!("../app.template.html");
const REFRESH: Duration = Duration::from_secs(48 * 60 * 60); // every 2 days

const HEAD: &str = r#"<!doctype html>
<html lang="es-MX">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="description" content="Chiripa — apuestas del Melate generadas con 8 estrategias estadísticas sobre el histórico oficial. Sitio no oficial, solo entretenimiento: toda combinación tiene la misma probabilidad.">
<link rel="icon" href="data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><text y='.9em' font-size='90'>🍀</text></svg>">
</head>
<body>
"#;
const FOOT: &str = "\n</body>\n</html>\n";

#[derive(Default)]
struct AppState {
    page: String,
    stats_json: String,
    stats: Option<Arc<Stats>>,
    next_pot: Option<u64>,
    updated: u64,
    downloads: u32,
    tigris_backup: bool,
}

type Shared = Arc<RwLock<AppState>>;

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn stats_json(st: &Stats, draws: &[Draw]) -> String {
    let today = (now() / 86_400) as i64;
    let (next_date, next_weekday) = next_draw(draws, today).unwrap_or_default();
    let per = |f: &dyn Fn(u8) -> serde_json::Value| -> Vec<serde_json::Value> {
        (1..=56u8).map(f).collect()
    };
    let rank = st.rank();
    serde_json::json!({
        "draw": st.contest,
        "date": st.date,
        "last": st.last,
        "draws": st.n,
        "window": WINDOW,
        "freq": per(&|n| st.freq[n as usize].into()),
        "recent": per(&|n| st.recent[n as usize].into()),
        "gap": per(&|n| st.gap[n as usize].into()),
        "rank": per(&|n| rank[n as usize].into()),
        "pop": per(&|n| Stats::popularity(n).into()),
        "balanced_w": per(&|n| st.balanced_weight(n).into()),
        "best_pair": per(&|n| { let (m, v) = st.best_pair(n); serde_json::json!([m, v]) }),
        "sum_lo": st.sum_lo,
        "sum_hi": st.sum_hi,
        "max_gap": st.max_gap,
        "f_den": st.f_den(),
        "r_den": st.r_den(),
        "hot6": generate(st, "hot", 6, &mut rand::rng()),
        "cold6": generate(st, "cold", 6, &mut rand::rng()),
        "pot": st.last_pot,
        "next_date": next_date,
        "next_weekday": next_weekday,
    })
    .to_string()
}

/// Rebuild page and stats from a CSV. Returns false if it looks corrupt.
fn publish(state: &Shared, csv: &str, downloaded: bool) -> bool {
    let draws = parse(csv);
    if draws.len() < 100 {
        return false;
    }
    let st = Stats::new(&draws);
    let page = TEMPLATE
        .replace("__LAST_DRAW__", &st.contest.to_string())
        .replace("__LAST_DATE__", &st.date);
    let json = stats_json(&st, &draws);
    let mut s = state.write().unwrap();
    s.page = format!("{HEAD}{page}{FOOT}");
    s.stats_json = json;
    s.updated = now();
    if downloaded {
        s.downloads += 1;
    }
    tracing::info!(contest = st.contest, date = %st.date, "page ready");
    s.stats = Some(Arc::new(st));
    true
}

struct TigrisStore {
    bucket: Bucket,
    credentials: Credentials,
}

impl TigrisStore {
    fn from_env() -> Option<Self> {
        let endpoint: String = std::env::var("AWS_ENDPOINT_URL_S3").ok()?;
        let name = std::env::var("BUCKET_NAME").ok()?;
        let region = std::env::var("AWS_REGION").unwrap_or_else(|_| "auto".into());
        let key = std::env::var("AWS_ACCESS_KEY_ID").ok()?;
        let secret = std::env::var("AWS_SECRET_ACCESS_KEY").ok()?;
        let bucket = Bucket::new(endpoint.parse().ok()?, UrlStyle::Path, name, region).ok()?;
        Some(Self { bucket, credentials: Credentials::new(key, secret) })
    }

    async fn upload(&self, http: &reqwest::Client, csv: &str) -> Result<()> {
        let action = self.bucket.put_object(Some(&self.credentials), "Melate.csv");
        let url = action.sign(Duration::from_secs(300));
        http.put(url).body(csv.to_owned()).send().await?.error_for_status()?;
        Ok(())
    }

    async fn download(&self, http: &reqwest::Client) -> Result<String> {
        let action = self.bucket.get_object(Some(&self.credentials), "Melate.csv");
        let url = action.sign(Duration::from_secs(300));
        Ok(http.get(url).send().await?.error_for_status()?.text().await?)
    }
}

async fn fetch_official(http: &reqwest::Client) -> Result<String> {
    Ok(http.get(CSV_URL).send().await?.error_for_status()?.text().await?)
}

/// Advertised Melate+Revancha+Revanchita pot for the upcoming draw, scraped
/// from the Lotería Nacional homepage carousel.
async fn fetch_advertised_pot(http: &reqwest::Client) -> Result<Option<u64>> {
    let html = http.get(POT_URL).send().await?.error_for_status()?.text().await?;
    Ok(extract_pot(&html))
}

async fn refresher(state: Shared, http: reqwest::Client, tigris: Option<TigrisStore>) {
    loop {
        match fetch_advertised_pot(&http).await {
            Ok(Some(pot)) => {
                state.write().unwrap().next_pot = Some(pot);
                tracing::info!(pot, "advertised pot updated");
            }
            Ok(None) => tracing::warn!("pot carousel not found in homepage HTML"),
            Err(e) => tracing::warn!("pot fetch failed: {e:#}"),
        }
        match fetch_official(&http).await {
            Ok(csv) if publish(&state, &csv, true) => {
                if let Some(t) = &tigris {
                    match t.upload(&http, &csv).await {
                        Ok(()) => {
                            state.write().unwrap().tigris_backup = true;
                            tracing::info!("CSV backed up to Tigris");
                        }
                        Err(e) => tracing::warn!("Tigris backup failed: {e:#}"),
                    }
                }
            }
            result => {
                if let Err(e) = result {
                    tracing::warn!("official download failed: {e:#}");
                } else {
                    tracing::warn!("downloaded CSV failed validation");
                }
                let empty = state.read().unwrap().page.is_empty();
                if empty {
                    if let Some(t) = &tigris {
                        match t.download(&http).await {
                            Ok(csv) => {
                                publish(&state, &csv, false);
                                tracing::info!("serving from the Tigris backup");
                            }
                            Err(e) => tracing::warn!("Tigris backup unavailable too: {e:#}"),
                        }
                    }
                }
            }
        }
        tokio::time::sleep(REFRESH).await;
    }
}

async fn index(State(state): State<Shared>) -> Html<String> {
    Html(state.read().unwrap().page.clone())
}

async fn health(State(state): State<Shared>) -> Json<serde_json::Value> {
    let s = state.read().unwrap();
    let (contest, date) = s
        .stats
        .as_ref()
        .map(|st| (st.contest, st.date.clone()))
        .unwrap_or_default();
    Json(serde_json::json!({
        "ok": !s.page.is_empty(),
        "draw": contest,
        "date": date,
        "updated_secs_ago": now().saturating_sub(s.updated),
        "downloads": s.downloads,
        "tigris_backup": s.tigris_backup,
    }))
}

async fn api_stats(State(state): State<Shared>) -> impl IntoResponse {
    let (body, next_pot) = {
        let s = state.read().unwrap();
        (s.stats_json.clone(), s.next_pot)
    };
    let mut v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    v["next_pot"] = next_pot.map(Into::into).unwrap_or(serde_json::Value::Null);
    ([(header::CONTENT_TYPE, "application/json")], v.to_string())
}

async fn api_tickets(
    State(state): State<Shared>,
    Query(q): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let Some(st) = state.read().unwrap().stats.clone() else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"error": "warming up"})));
    };
    let requested: Vec<String> = q
        .get("strategies")
        .map(|s| s.split(',').map(str::to_owned).collect())
        .unwrap_or_else(|| STRATEGIES.iter().map(|s| s.to_string()).collect());
    let selection: Vec<&str> = STRATEGIES
        .into_iter()
        .filter(|id| requested.iter().any(|r| r == id))
        .collect();
    if selection.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"error": "unknown strategies"})));
    }
    let count: usize = q.get("count").and_then(|v| v.parse().ok()).unwrap_or(8).clamp(1, 12);
    let k: usize = q.get("k").and_then(|v| v.parse().ok()).unwrap_or(6).clamp(6, 10);

    let mut rng = rand::rng();
    let tickets: Vec<serde_json::Value> = plan(&selection, count)
        .into_iter()
        .map(|id| {
            let numbers = generate(&st, id, k, &mut rng);
            let cooc_sum: Vec<u32> = numbers
                .iter()
                .map(|&n| numbers.iter().filter(|&&m| m != n).map(|&m| st.cooc(n, m)).sum())
                .collect();
            serde_json::json!({ "strategy": id, "numbers": numbers, "cooc_sum": cooc_sum })
        })
        .collect();
    (StatusCode::OK, Json(serde_json::json!({ "k": k, "tickets": tickets })))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_target(false).init();

    let state: Shared = Arc::default();
    publish(&state, SEED, false); // instant boot from the embedded seed

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .user_agent("chiripa.mx/1.0")
        .build()?;
    let tigris = TigrisStore::from_env();
    tracing::info!(tigris = tigris.is_some(), "bucket backup configured");
    tokio::spawn(refresher(state.clone(), http, tigris));

    let app = Router::new()
        .route("/", get(index))
        .route("/health", get(health))
        .route("/api/stats", get(api_stats))
        .route("/api/tickets", get(api_tickets))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    tracing::info!("chiripa listening on :8080");
    axum::serve(listener, app).await?;
    Ok(())
}
