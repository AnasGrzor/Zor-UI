//! Rust port of scripts/football.py (ESPN fixtures, win/draw/goal/corner odds, standings, form, live
//! scores), which comes from end4-pC by pctrade (GPL-3.0, (c) end-4 / pctrade). It sits behind the bridge `football` command. Responses are cached under
//! %LOCALAPPDATA%\zor-ui\cache\football, in the same files and formats the script used.

use super::common::*;
use super::*;
use serde_json::{json, Map};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{atomic::{AtomicUsize, Ordering}, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0 Safari/537.36";
const SITE: &str = "https://site.api.espn.com/apis/site/v2/sports/soccer";
const CORE: &str = "https://sports.core.api.espn.com/v2/sports/soccer/leagues";
/// (id, name, country, league goals per team per game, domestic).
const LEAGUES: [(&str, &str, &str, f64, bool); 14] = [
    ("esp.1", "LaLiga", "ES", 1.30, true),
    ("eng.1", "Premier League", "EN", 1.45, true),
    ("ita.1", "Serie A", "IT", 1.30, true),
    ("fra.1", "Ligue 1", "FR", 1.40, true),
    ("por.1", "Primeira Liga", "PT", 1.30, true),
    ("tur.1", "Süper Lig", "TR", 1.45, true),
    ("ned.1", "Eredivisie", "NL", 1.65, true),
    ("ger.1", "Bundesliga", "DE", 1.60, true),
    ("mex.1", "Liga MX", "MX", 1.40, true),
    ("arg.1", "Liga Profesional", "AR", 1.10, true),
    ("bra.1", "Brasileirão", "BR", 1.25, true),
    ("ksa.1", "Saudi Pro League", "SA", 1.45, true),
    ("uefa.champions", "Champions League", "EU", 1.45, false),
    ("uefa.europa", "Europa League", "EU", 1.40, false),
];
const CORNERS_PER_TEAM: f64 = 5.0;
const SHRINK: f64 = 5.0;
const HOME_BOOST: f64 = 1.10;
const AWAY_FACTOR: f64 = 0.92;
const DAYS_AHEAD: i64 = 8;

/// One-shot command `name`; None if this module doesn't port it.
pub fn call(_cfg: &Config, name: &str, args: &[&str]) -> Option<Value> {
    if name != "football" { return None; }
    let after = |flag: &str, n: usize| args.iter().position(|a| *a == flag).and_then(|i| args.get(i + n).copied());
    let has = |flag: &str| args.contains(&flag);
    Some(if has("--form") {
        match (after("--form", 1), after("--form", 2)) {
            (Some(l), Some(t)) => team_form(l, &t.split(',').map(String::from).collect::<Vec<_>>(), 10),
            _ => err("--form needs a league and team ids"),
        }
    } else if has("--positions") { positions() }
    else if has("--standings") { match after("--standings", 1) { Some(l) => standings(l), None => err("--standings needs a league") } }
    else if has("--live") { live_update() }
    else { main_matches() })
}

// ---------------------------------------------------------------- plumbing

fn cache_dir() -> PathBuf { root().join("cache").join("football") }

fn read_cache(path: &std::path::Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn write_cache(path: &std::path::Path, v: &Value) {
    if let Some(d) = path.parent() { let _ = std::fs::create_dir_all(d); }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, v.to_string()).is_ok() { let _ = std::fs::rename(&tmp, path); }
}

/// GET `url` as JSON, using the cache file `key` while it is younger than `ttl` seconds (negative: always
/// refetch) and falling back to a stale copy when the network fails.
fn fetch(url: &str, ttl: f64, key: &str) -> Option<Value> {
    let path = cache_dir().join(format!("{key}.json"));
    let age = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).map(|d| d.as_secs_f64());
    if age.is_some_and(|a| a < ttl) {
        if let Some(v) = read_cache(&path) { return Some(v); }
    }
    match get_bytes(url, &[("User-Agent", UA)], 15).and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
        Some(v) => { write_cache(&path, &v); Some(v) }
        None => read_cache(&path),
    }
}

/// `f` over `items` on up to `workers` threads; results keep the input order.
fn pmap<T: Sync, R: Send>(items: &[T], workers: usize, f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let out = Mutex::new(Vec::with_capacity(items.len()));
    std::thread::scope(|s| {
        for _ in 0..workers.min(items.len()) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(item) = items.get(i) else { break };
                let r = f(item);
                if let Ok(mut o) = out.lock() { o.push((i, r)); }
            });
        }
    });
    let mut v = out.into_inner().unwrap_or_default();
    v.sort_by_key(|(i, _)| *i);
    v.into_iter().map(|(_, r)| r).collect()
}

/// Python's `float(v or 0)`: numbers and numeric strings, anything else 0.
fn num(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.trim().parse().unwrap_or(0.0),
        Value::Bool(true) => 1.0,
        _ => 0.0,
    }
}
fn int(v: &Value) -> i64 { num(v) as i64 }
/// Python's str() of a JSON id/season (strings as is, numbers without a decimal point).
fn text(v: &Value) -> String { v.as_str().map(String::from).unwrap_or_else(|| if v.is_null() { String::new() } else { v.to_string() }) }
fn s(v: &Value) -> &str { v.as_str().unwrap_or("") }
fn arr(v: &Value) -> &[Value] { v.as_array().map(Vec::as_slice).unwrap_or(&[]) }
/// `{stat name: value}` from an ESPN `stats` list.
fn flat(stats: &Value) -> HashMap<&str, &Value> {
    arr(stats).iter().filter_map(|st| Some((st["name"].as_str()?, &st["value"]))).collect()
}

fn league_by_id(id: &str) -> Option<&'static (&'static str, &'static str, &'static str, f64, bool)> { LEAGUES.iter().find(|l| l.0 == id) }

// ---------------------------------------------------------------- dates

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn stamp(days: i64) -> String { let (y, m, d) = civil_from_days(days); format!("{y:04}{m:02}{d:02}") }

/// Seconds since the epoch for an ISO-8601 time like `2026-11-04T15:30Z` or `...:00.000+01:00`.
pub(super) fn parse_iso(v: &str) -> Option<i64> {
    let (date, time) = v.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let tpos = time.find(['Z', '+', '-']).unwrap_or(time.len());
    let mut t = time[..tpos].split(':');
    let h: i64 = t.next()?.parse().ok()?;
    let mi: i64 = t.next()?.parse().ok()?;
    let sec = t.next().and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0) as i64;
    let tz = &time[tpos..];
    let off = match tz.chars().next() {
        Some(c @ ('+' | '-')) => {
            let mut p = tz[1..].split(':');
            let secs = p.next().and_then(|x| x.parse::<i64>().ok()).unwrap_or(0) * 3600 + p.next().and_then(|x| x.parse::<i64>().ok()).unwrap_or(0) * 60;
            if c == '+' { secs } else { -secs }
        }
        _ => 0,
    };
    Some(days_from_civil(y, m, day) * 86_400 + h * 3600 + mi * 60 + sec - off)
}

fn now_unix() -> f64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs_f64() }

/// Python's `datetime.now(utc).isoformat()`.
fn iso_utc(t: f64) -> String {
    let secs = t.floor() as i64;
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    let r = secs.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:06}+00:00", r / 3600, r % 3600 / 60, r % 60, ((t - t.floor()) * 1e6) as i64)
}

// ---------------------------------------------------------------- maths

fn poisson_pmf(mean: f64, kmax: usize) -> Vec<f64> {
    let mut out = vec![(-mean).exp()];
    for k in 1..=kmax { out.push(out[k - 1] * mean / k as f64); }
    out
}

/// (home, draw, away, over 1.5, over 2.5) from two Poisson means with the Dixon-Coles low-score correction.
fn goals_probs(lh: f64, la: f64) -> (f64, f64, f64, f64, f64) {
    let (ph, pa) = (poisson_pmf(lh, 10), poisson_pmf(la, 10));
    let rho = -0.08;
    let mut m = [[0.0f64; 11]; 11];
    for i in 0..11 { for j in 0..11 { m[i][j] = ph[i] * pa[j]; } }
    m[0][0] *= 1.0 - lh * la * rho;
    m[0][1] *= 1.0 + lh * rho;
    m[1][0] *= 1.0 + la * rho;
    m[1][1] *= 1.0 - rho;
    let total: f64 = m.iter().map(|r| r.iter().sum::<f64>()).sum();
    let (mut home, mut draw, mut away, mut o15, mut o25) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for i in 0..11 {
        for j in 0..11 {
            let p = m[i][j] / total;
            if i > j { home += p } else if i == j { draw += p } else { away += p }
            if i + j >= 2 { o15 += p }
            if i + j >= 3 { o25 += p }
        }
    }
    (home, draw, away, o15, o25)
}

/// P(corners > line) for a negative binomial with variance = mean * 1.35 (pmf by recurrence: no lgamma in std).
fn negbin_over(mean: f64, line: f64) -> f64 {
    let r = mean / (1.35 - 1.0);
    let p = r / (r + mean);
    let (mut pmf, mut cdf) = (p.powf(r), 0.0);
    for k in 0..=(line.floor() as usize) {
        cdf += pmf;
        pmf *= (k as f64 + r) / (k as f64 + 1.0) * (1.0 - p);
    }
    (1.0 - cdf).clamp(0.0, 1.0)
}

fn shrink(per_game: f64, games: f64, prior: f64) -> f64 { (per_game * games + prior * SHRINK) / (games + SHRINK) }

/// Python's round(): formatting rounds the exact binary value, unlike x * 10^n.
fn round(x: f64, digits: usize) -> f64 { format!("{x:.digits$}").parse().unwrap_or(x) }

// ---------------------------------------------------------------- team stats

#[derive(Clone, Copy)]
struct Stats { games: f64, gf: f64, ga: f64, cf: f64, ca: f64 }

fn team_stats(league: &str, season: &str, team: &str) -> Option<Stats> {
    let data = fetch(&format!("{CORE}/{league}/seasons/{season}/types/1/teams/{team}/statistics"), 12.0 * 3600.0, &format!("stats_{league}_{season}_{team}"));
    let mut f: HashMap<&str, &Value> = HashMap::new();
    if let Some(d) = &data {
        for cat in arr(&d["splits"]["categories"]) { f.extend(flat(&cat["stats"])); }
    }
    let g = |k: &str| f.get(k).map(|v| num(v)).unwrap_or(0.0);
    let games = g("appearances");
    (games > 0.0).then(|| Stats { games, gf: g("totalGoals") / games, ga: g("goalsConceded") / games, cf: g("wonCorners") / games, ca: g("lostCorners") / games })
}

/// team id -> domestic league id.
fn domestic_index() -> HashMap<String, &'static str> {
    let doms: Vec<_> = LEAGUES.iter().filter(|l| l.4).collect();
    let data = pmap(&doms, 8, |l| fetch(&format!("{SITE}/{}/teams?limit=100", l.0), 7.0 * 86400.0, &format!("teams_{}", l.0)));
    let mut index = HashMap::new();
    for (l, d) in doms.iter().zip(&data) {
        let Some(d) = d else { continue };
        for item in arr(&d["sports"][0]["leagues"][0]["teams"]) {
            if !item["team"]["id"].is_null() { index.insert(text(&item["team"]["id"]), l.0); }
        }
    }
    index
}

// ---------------------------------------------------------------- main fixtures list

fn main_matches() -> Value {
    let n = crate::util::now();
    let today = days_from_civil(n.wYear as i64, n.wMonth as i64, n.wDay as i64);
    let jobs: Vec<(usize, String)> = (0..LEAGUES.len()).flat_map(|li| (0..=DAYS_AHEAD).map(move |o| (li, stamp(today + o)))).collect();
    let boards = pmap(&jobs, 16, |(li, st)| fetch(&format!("{SITE}/{}/scoreboard?dates={st}", LEAGUES[*li].0), 20.0 * 60.0, &format!("board_{}_{st}", LEAGUES[*li].0)));

    // events keep first-seen order, like the script's dict
    let mut events: Vec<(usize, &Value)> = vec![];
    let mut pos: HashMap<(usize, String), usize> = HashMap::new();
    let mut seasons: HashMap<&str, String> = HashMap::new();
    for ((li, _), data) in jobs.iter().zip(&boards) {
        let Some(data) = data else { continue };
        let year = &data["leagues"][0]["season"]["year"];
        if !year.is_null() { seasons.insert(LEAGUES[*li].0, text(year)); }
        for ev in arr(&data["events"]) {
            let key = (*li, text(&ev["id"]));
            match pos.get(&key) { Some(&i) => events[i] = (*li, ev), None => { pos.insert(key, events.len()); events.push((*li, ev)); } }
        }
    }
    let domestic = domestic_index();

    let mut needed: Vec<(String, String, String)> = vec![];
    let mut seen = std::collections::HashSet::new();
    for (li, ev) in &events {
        let lid = LEAGUES[*li].0;
        for c in arr(&ev["competitions"][0]["competitors"]) {
            let team = text(&c["team"]["id"]);
            let Some(season) = seasons.get(lid) else { continue };
            let mut add = |k: (String, String, String)| if seen.insert(k.clone()) { needed.push(k) };
            add((lid.into(), season.clone(), team.clone()));
            if let Some(&hl) = domestic.get(&team) {
                if hl != lid { add((hl.into(), seasons.get(hl).unwrap_or(season).clone(), team)); }
            }
        }
    }
    let stats: HashMap<_, _> = needed.iter().cloned().zip(pmap(&needed, 16, |(l, se, t)| team_stats(l, se, t))).collect();

    let best = |lid: &str, team: &str| -> (Option<Stats>, f64) {
        let season = seasons.get(lid);
        let get = |l: &str, se: Option<&String>| se.and_then(|se| stats.get(&(l.to_string(), se.clone(), team.to_string())).copied().flatten());
        let own = get(lid, season);
        if let Some(&hl) = domestic.get(team) {
            if let Some(d) = get(hl, seasons.get(hl).or(season)) {
                if own.map_or(true, |o| d.games >= o.games) { return (Some(d), league_by_id(hl).map_or(1.0, |l| l.3)); }
            }
        }
        (own, league_by_id(lid).map_or(1.0, |l| l.3))
    };

    let mut matches: Vec<Value> = vec![];
    for (li, ev) in &events {
        let (lid, lname, lcountry, avg, _) = LEAGUES[*li];
        let comps = arr(&ev["competitions"][0]["competitors"]);
        let side = |ha: &str| comps.iter().find(|c| c["homeAway"] == ha);
        let (Some(hc), Some(ac)) = (side("home"), side("away")) else { continue };
        let status = &ev["status"]["type"];
        let mut row = json!({
            "id": ev["id"], "league": {"id": lid, "name": lname, "country": lcountry}, "date": ev["date"],
            "state": status.get("state").cloned().unwrap_or(json!("pre")),
            "detail": status.get("shortDetail").cloned().unwrap_or(json!("")),
        });
        for (name, c) in [("home", hc), ("away", ac)] {
            let t = &c["team"];
            row[name] = json!({
                "id": text(&t["id"]), "name": t.get("displayName").cloned().unwrap_or(json!("")), "abbr": t.get("abbreviation").cloned().unwrap_or(json!("")),
                "logo": t.get("logo").cloned().unwrap_or(json!("")), "score": c["score"],
            });
        }
        let (hs, h_avg) = best(lid, &text(&hc["team"]["id"]));
        let (as_, a_avg) = best(lid, &text(&ac["team"]["id"]));
        let (hg, ag) = (hs.map_or(0, |x| x.games as i64), as_.map_or(0, |x| x.games as i64));
        row["sample"] = json!({"home": hg, "away": ag});
        row["lowData"] = json!(hg.min(ag) < 4);
        if let (Some(h), Some(a)) = (hs, as_) {
            let att_h = shrink(h.gf, h.games, h_avg) / h_avg;
            let def_h = shrink(h.ga, h.games, h_avg) / h_avg;
            let att_a = shrink(a.gf, a.games, a_avg) / a_avg;
            let def_a = shrink(a.ga, a.games, a_avg) / a_avg;
            let lam_h = (avg * att_h * def_a * HOME_BOOST).max(0.35).min(3.8);
            let lam_a = (avg * att_a * def_h * AWAY_FACTOR).max(0.35).min(3.8);
            let (home, draw, away, o15, o25) = goals_probs(lam_h, lam_a);
            let sc = |v: f64, g: f64| shrink(v, g, CORNERS_PER_TEAM) / CORNERS_PER_TEAM;
            let exp_c = (CORNERS_PER_TEAM * 1.06 * sc(h.cf, h.games) * sc(a.ca, a.games) + CORNERS_PER_TEAM * 0.94 * sc(a.cf, a.games) * sc(h.ca, h.games)).max(6.0).min(14.0);
            row["probs"] = json!({
                "home": round(home, 4), "draw": round(draw, 4), "away": round(away, 4), "o15": round(o15, 4), "o25": round(o25, 4),
                "c85": round(negbin_over(exp_c, 8.5), 4), "c95": round(negbin_over(exp_c, 9.5), 4), "c105": round(negbin_over(exp_c, 10.5), 4),
            });
            row["expected"] = json!({"home": round(lam_h, 2), "away": round(lam_a, 2), "corners": round(exp_c, 1)});
        }
        matches.push(row);
    }
    matches.sort_by(|a, b| s(&a["date"]).cmp(s(&b["date"])));
    let out = json!({
        "generated": iso_utc(now_unix()),
        "leagues": LEAGUES.iter().map(|l| json!({"id": l.0, "name": l.1, "country": l.2})).collect::<Vec<_>>(),
        "matches": matches,
    });
    write_cache(&cache_dir().join("matches.json"), &out);
    out
}

// ---------------------------------------------------------------- standings, positions

fn standings_data(league: &str) -> Option<Value> {
    fetch(&format!("https://site.api.espn.com/apis/v2/sports/soccer/{league}/standings"), 3.0 * 3600.0, &format!("standings_{league}"))
}

fn positions() -> Value {
    let all = pmap(&LEAGUES, 8, |l| {
        let mut ranks = Map::new();
        if let Some(d) = standings_data(l.0) {
            for child in arr(&d["children"]) {
                for e in arr(&child["standings"]["entries"]) {
                    let rank = int(flat(&e["stats"]).get("rank").copied().unwrap_or(&Value::Null));
                    if rank > 0 { ranks.insert(text(&e["team"]["id"]), json!(rank)); }
                }
            }
        }
        (l.0.to_string(), Value::Object(ranks))
    });
    Value::Object(all.into_iter().collect())
}

fn standings(league: &str) -> Value {
    let data = standings_data(league);
    let mut groups = vec![];
    for child in data.iter().flat_map(|d| arr(&d["children"])) {
        let mut rows: Vec<Value> = arr(&child["standings"]["entries"]).iter().map(|e| {
            let f = flat(&e["stats"]);
            let g = |k: &str| int(f.get(k).copied().unwrap_or(&Value::Null));
            let t = &e["team"];
            json!({
                "rank": g("rank"), "id": text(&t["id"]), "name": s(&t["displayName"]), "abbr": s(&t["abbreviation"]),
                "logo": s(&t["logos"][0]["href"]), "p": g("gamesPlayed"), "w": g("wins"), "d": g("ties"), "l": g("losses"),
                "gf": g("pointsFor"), "ga": g("pointsAgainst"), "gd": g("pointDifferential"), "pts": g("points"),
            })
        }).collect();
        rows.sort_by_key(|r| match r["rank"].as_i64() { Some(0) | None => 999, Some(x) => x });
        groups.push(json!({"name": s(&child["name"]), "rows": rows}));
    }
    json!({"league": league, "groups": groups})
}

// ---------------------------------------------------------------- form

/// Corner counts of a finished match, cached forever: [(team id, corners)] (empty unless both teams are known).
fn event_corners(league: &str, event: &str) -> Vec<(String, i64)> {
    let path = cache_dir().join(format!("corners_{event}.json"));
    if let Some(Value::Object(m)) = read_cache(&path) { return m.iter().map(|(k, v)| (k.clone(), int(v))).collect(); }
    let Some(data) = get_bytes(&format!("{SITE}/{league}/summary?event={event}"), &[("User-Agent", UA)], 15).and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { return vec![] };
    let mut found: Vec<(String, i64)> = vec![];
    for t in arr(&data["boxscore"]["teams"]) {
        for st in arr(&t["statistics"]) {
            if st["name"] == "wonCorners" {
                let (id, c) = (text(&t["team"]["id"]), int(&st["displayValue"]));
                match found.iter_mut().find(|f| f.0 == id) { Some(f) => f.1 = c, None => found.push((id, c)) }
            }
        }
    }
    if found.len() == 2 { write_cache(&path, &Value::Object(found.iter().map(|(k, v)| (k.clone(), json!(v))).collect())); }
    found
}

/// One finished game of `team` from an ESPN schedule event, or None if it is unfinished or malformed.
fn form_game(event: &Value, team: &str, home_league: &str) -> Option<Value> {
    let comp = &event["competitions"][0];
    if comp["status"]["type"]["state"] != "post" { return None; }
    let comps = arr(&comp["competitors"]);
    let mine = comps.iter().rev().find(|c| text(&c["team"]["id"]) == team)?;
    let other = comps.iter().find(|c| text(&c["team"]["id"]) != team)?;
    let score = |c: &Value| match &c["score"] { Value::Object(o) => int(o.get("value").or(o.get("displayValue")).unwrap_or(&Value::Null)), v => int(v) };
    let (ms, os) = (score(mine), score(other));
    Some(json!({
        "date": event.get("date")?, "r": if ms > os { "W" } else if ms < os { "L" } else { "D" },
        "vs": s(&other["team"]["abbreviation"]), "score": format!("{ms}-{os}"), "gf": ms, "ga": os, "event": event.get("id")?,
        "league": event["league"]["slug"].as_str().filter(|x| !x.is_empty()).unwrap_or(home_league),
    }))
}

fn team_form(league: &str, team_ids: &[String], count: usize) -> Value {
    let domestic = domestic_index();
    let all = pmap(team_ids, 4, |team| {
        let hl = domestic.get(team).copied().unwrap_or(league);
        let data = fetch(&format!("{SITE}/{hl}/teams/{team}/schedule"), 3.0 * 3600.0, &format!("schedule_{hl}_{team}"));
        let mut games: Vec<Value> = data.iter().flat_map(|d| arr(&d["events"])).filter_map(|e| form_game(e, team, hl)).collect();
        games.sort_by(|a, b| s(&a["date"]).cmp(s(&b["date"])));
        let skip = games.len().saturating_sub(count);
        let games = games.split_off(skip);
        let games = pmap(&games, 5, |g| {
            let mut g = g.clone();
            let found = event_corners(s(&g["league"]), &text(&g["event"]));
            let other = found.iter().find(|(k, _)| k != team);
            if let (Some(cf), Some(ca)) = (found.iter().find(|(k, _)| k == team), other) { g["cf"] = json!(cf.1); g["ca"] = json!(ca.1); }
            g
        });
        (team.clone(), Value::Array(games))
    });
    Value::Object(all.into_iter().collect())
}

// ---------------------------------------------------------------- live

fn match_corners(league: &str, event: &str) -> Value {
    let mut r = json!({"home": null, "away": null});
    if let Some(d) = fetch(&format!("{SITE}/{league}/summary?event={event}"), -1.0, &format!("summary_{event}")) {
        for t in arr(&d["boxscore"]["teams"]) {
            for st in arr(&t["statistics"]) {
                if st["name"] == "wonCorners" { if let Some(ha) = t["homeAway"].as_str() { r[ha] = json!(int(&st["displayValue"])); } }
            }
        }
    }
    r
}

fn live_update() -> Value {
    let path = cache_dir().join("matches.json");
    let Some(mut cached) = read_cache(&path) else { return json!({"updates": []}) };
    let now = now_unix();
    let mut pending: Vec<String> = vec![];
    for m in arr(&cached["matches"]) {
        let kick = parse_iso(s(&m["date"])).map_or(f64::MIN, |k| k as f64);
        if m["state"] == "in" || (m["state"] == "pre" && now - 4.0 * 3600.0 < kick && kick <= now + 120.0) {
            let l = text(&m["league"]["id"]);
            if !pending.contains(&l) { pending.push(l); }
        }
    }
    pending.sort();
    let mut index: HashMap<String, usize> = HashMap::new();
    for (i, m) in arr(&cached["matches"]).iter().enumerate() { index.insert(text(&m["id"]), i); }
    let today = (now / 86400.0).floor() as i64;
    let stamps = [stamp(today), stamp(today - 1)];
    let jobs: Vec<(&String, &String)> = pending.iter().flat_map(|l| stamps.iter().map(move |s| (l, s))).collect();
    let boards = pmap(&jobs, 8, |(l, st)| fetch(&format!("{SITE}/{l}/scoreboard?dates={st}"), -1.0, &format!("live_{l}_{st}")));

    let mut updates: Vec<(String, String)> = vec![];
    for ((lid, _), data) in jobs.iter().zip(&boards) {
        for ev in data.iter().flat_map(|d| arr(&d["events"])) {
            let id = text(&ev["id"]);
            let Some(&i) = index.get(&id) else { continue };
            let comps = arr(&ev["competitions"][0]["competitors"]);
            let side = |ha: &str| comps.iter().find(|c| c["homeAway"] == ha);
            let (Some(h), Some(a)) = (side("home"), side("away")) else { continue };
            let status = &ev["status"]["type"];
            let Some(known) = cached["matches"].get_mut(i) else { continue };
            if let Some(st) = status.get("state") { known["state"] = st.clone(); }
            known["detail"] = status.get("shortDetail").cloned().unwrap_or(json!(""));
            known["home"]["score"] = h["score"].clone();
            known["away"]["score"] = a["score"].clone();
            updates.push((id, lid.to_string()));
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut results = vec![];
    for (id, lid) in updates {
        if !seen.insert(id.clone()) { continue; }
        let m = &cached["matches"][index[&id]];
        let corners = if m["state"] == "in" { match_corners(&lid, &id) } else { json!({"home": null, "away": null}) };
        results.push(json!({"id": m["id"], "state": m["state"], "detail": m["detail"], "home": {"score": m["home"]["score"]}, "away": {"score": m["away"]["score"]}, "corners": corners}));
    }
    if !results.is_empty() { write_cache(&path, &cached); }
    json!({"updates": results})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) { assert!((a - b).abs() < 1e-12, "{a} vs {b}"); }

    // Expected values come from football.py's poisson_pmf / goals_probs / negbin_over.
    #[test]
    fn poisson_matches_python() {
        let p = poisson_pmf(1.5, 10);
        close(p[0], 0.22313016014842982);
        close(p[3], 0.12551071508349176);
        close(p[10], 3.545747740570184e-06);
    }

    #[test]
    fn goals_probs_matches_python() {
        let (h, d, a, o15, o25) = goals_probs(1.6, 1.1);
        close(h, 0.4801104806616566);
        close(d, 0.26783633454717737);
        close(a, 0.252053184791166);
        close(o15, 0.7608018876395841);
        close(o25, 0.5063749900856216);
    }

    #[test]
    fn negbin_matches_python() {
        close(negbin_over(10.0, 8.5), 0.6340756796485257);
        close(negbin_over(9.0, 10.5), 0.30962929438966824);
    }

    #[test]
    fn dates() {
        assert_eq!(parse_iso("2026-11-04T15:30Z"), Some(days_from_civil(2026, 11, 4) * 86_400 + 15 * 3600 + 1800));
        assert_eq!(parse_iso("2026-11-04T15:30:00+01:00"), Some(days_from_civil(2026, 11, 4) * 86_400 + 14 * 3600 + 1800));
        assert_eq!(stamp(days_from_civil(2024, 2, 29)), "20240229");
        assert_eq!(iso_utc(0.5), "1970-01-01T00:00:00.500000+00:00");
    }

    #[test]
    fn form_game_parses() {
        let ev = json!({"id": "9", "date": "2026-01-01T00:00Z", "competitions": [{"status": {"type": {"state": "post"}}, "competitors": [
            {"team": {"id": "1", "abbreviation": "AAA"}, "score": {"value": 2.0}}, {"team": {"id": "2", "abbreviation": "BBB"}, "score": "3"}]}]});
        let g = form_game(&ev, "1", "esp.1").unwrap();
        assert_eq!((g["r"].as_str(), g["score"].as_str(), g["vs"].as_str(), g["league"].as_str()), (Some("L"), Some("2-3"), Some("BBB"), Some("esp.1")));
    }
}
