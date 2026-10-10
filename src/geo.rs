use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GeoPoint {
    pub lat: f64,
    pub lon: f64,
}

impl GeoPoint {
    /// Понимает координаты в том виде, в каком их копируют из Яндекс Карт: «55.751244, 37.618423».
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split([',', ';', ' ']).filter(|part| !part.trim().is_empty());
        let lat: f64 = parts.next()?.trim().parse().ok()?;
        let lon: f64 = parts.next()?.trim().parse().ok()?;
        let valid = parts.next().is_none() && (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon);
        valid.then_some(Self { lat, lon })
    }
}

impl std::fmt::Display for GeoPoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:.6}, {:.6}", self.lat, self.lon)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_coordinates_copied_from_maps() {
        assert_eq!(
            GeoPoint::parse("55.751244, 37.618423"),
            Some(GeoPoint {
                lat: 55.751_244,
                lon: 37.618_423
            })
        );
        assert_eq!(GeoPoint::parse(" 55.75 37.61 ").map(|p| p.lon), Some(37.61));
        assert_eq!(GeoPoint::parse("55.75"), None);
        assert_eq!(GeoPoint::parse("155, 37"), None);
        assert_eq!(GeoPoint::parse("Москва"), None);
    }

    #[test]
    fn display_round_trips_through_parse() {
        let home = GeoPoint {
            lat: 55.729_777,
            lon: 37.640_123,
        };
        assert_eq!(GeoPoint::parse(&home.to_string()), Some(home));
    }
}
