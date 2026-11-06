//! Weather like Zebar's providers: location from ipinfo.io, current weather from Open-Meteo.

use crate::util::http_get;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Weather { pub status: &'static str, pub celsius: f64, pub wind: Option<f64>, pub city: String }

/// Zebar's mapping from WMO weather codes to its status names.
pub fn status(code: u64, day: bool) -> &'static str {
    let dn = |d: &'static str, n: &'static str| if day { d } else { n };
    match code {
        0 => dn("clear_day", "clear_night"),
        1..=50 => dn("cloudy_day", "cloudy_night"),
        51..=62 => dn("light_rain_day", "light_rain_night"),
        63..=70 | 80..=84 => dn("heavy_rain_day", "heavy_rain_night"),
        71..=79 | 85..=94 => dn("snow_day", "snow_night"),
        _ => dn("thunder_day", "thunder_night"),
    }
}

/// weatherIcon() in core.js.
pub fn icon(status: &str) -> &'static str {
    match status {
        "clear_day" => "sunny", "clear_night" => "clear_night",
        "cloudy_day" => "partly_cloudy_day", "cloudy_night" => "partly_cloudy_night",
        s if s.contains("rain") => "rainy",
        s if s.starts_with("snow") => "weather_snowy",
        s if s.starts_with("thunder") => "thunderstorm",
        _ => "cloud",
    }
}

/// weatherText() in core.js: "light_rain_day" -> "light rain", "clear_day" -> "clear sky".
pub fn text(status: &str) -> String {
    let s = status.trim_end_matches("_day").trim_end_matches("_night").replace('_', " ");
    if s == "clear" { "clear sky".into() } else { s }
}

/// temp() in core.js.
pub fn temp(w: &Weather, unit: &str) -> String {
    if unit == "F" { format!("{}°F", (w.celsius * 9.0 / 5.0 + 32.0).round()) } else { format!("{}°C", w.celsius.round()) }
}

pub fn fetch() -> Option<Weather> {
    let ip: Value = serde_json::from_str(&http_get("https://ipinfo.io/json")?).ok()?;
    let (lat, lon) = ip.get("loc")?.as_str()?.split_once(',')?;
    let url = format!("https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}&current_weather=true&timezone=auto");
    let w: Value = serde_json::from_str(&http_get(&url)?).ok()?;
    let cur = w.get("current_weather")?;
    Some(Weather {
        status: status(cur.get("weathercode")?.as_u64()?, cur.get("is_day").and_then(Value::as_u64) != Some(0)),
        celsius: cur.get("temperature")?.as_f64()?,
        wind: cur.get("windspeed").and_then(Value::as_f64),
        city: ip.get("city").and_then(Value::as_str).unwrap_or("").to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names() {
        assert_eq!(status(61, true), "light_rain_day");
        assert_eq!(status(0, false), "clear_night");
        assert_eq!(icon("heavy_rain_night"), "rainy");
        assert_eq!(text("clear_day"), "clear sky");
        assert_eq!(text("light_rain_night"), "light rain");
        let w = Weather { status: "clear_day", celsius: 21.6, wind: None, city: String::new() };
        assert_eq!(temp(&w, "F"), "71°F");
        assert_eq!(temp(&w, "C"), "22°C");
    }
}
