pub const MIN_ZOOM: f64 = 0.5;
pub const MAX_ZOOM: f64 = 5.0;
pub const DEFAULT_ZOOM: f64 = 1.0;

pub const LEVELS: &[f64] = &[
    0.5, 0.6, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.33, 1.5, 1.75, 2.0, 2.5,
    3.0, 4.0, 5.0,
];

pub fn clamp(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(MIN_ZOOM, MAX_ZOOM)
    } else {
        DEFAULT_ZOOM
    }
}

pub fn increase(current: f64) -> f64 {
    let current = clamp(current);
    LEVELS
        .iter()
        .copied()
        .find(|level| *level > current + 0.001)
        .unwrap_or(MAX_ZOOM)
}

pub fn decrease(current: f64) -> f64 {
    let current = clamp(current);
    LEVELS
        .iter()
        .rev()
        .copied()
        .find(|level| *level < current - 0.001)
        .unwrap_or(MIN_ZOOM)
}

pub fn origin(uri: &str) -> Option<String> {
    let (scheme, rest) = uri.split_once("://")?;
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{}", authority.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_steps_and_limits() {
        assert_eq!(increase(1.0), 1.1);
        assert_eq!(decrease(1.0), 0.9);
        assert_eq!(increase(5.0), 5.0);
        assert_eq!(decrease(0.5), 0.5);
        assert_eq!(clamp(f64::NAN), 1.0);
    }

    #[test]
    fn extracts_web_origins() {
        assert_eq!(
            origin("https://Example.COM:8443/a?q=1").as_deref(),
            Some("https://example.com:8443")
        );
        assert_eq!(origin("ubar://localhost/settings"), None);
        assert_eq!(origin("file:///tmp/a"), None);
    }
}
