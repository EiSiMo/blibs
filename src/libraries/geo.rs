//! Distances, computed offline from the coordinates in the list.
//!
//! There is no geocoder. `--near` takes coordinates or a library's alias; free text is a
//! usage error that says so, rather than a network round trip the user did not ask for.

/// Mean earth radius in kilometres (IUGG). The list's coordinates are given to five
/// decimals, so the model's error is far below the data's.
const EARTH_RADIUS_KM: f64 = 6371.0088;

/// A point on the earth.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct LatLon {
    /// Latitude in degrees.
    pub lat: f64,
    /// Longitude in degrees.
    pub lon: f64,
}

impl LatLon {
    /// A point from two degree values, rejecting anything that is not a point on the
    /// earth — `NaN`, an infinity, or a value outside the valid ranges.
    ///
    /// This is how a list entry without usable coordinates drops out of `--near` instead
    /// of sorting to the front of it.
    pub fn checked(lat: f64, lon: f64) -> Option<Self> {
        let plausible = lat.is_finite()
            && lon.is_finite()
            && (-90.0..=90.0).contains(&lat)
            && (-180.0..=180.0).contains(&lon);
        plausible.then_some(Self { lat, lon })
    }

    /// Parse a `lat,lon` pair as typed on the command line.
    ///
    /// Returns `None` for anything that is not two numbers separated by a comma — the
    /// caller turns that into the "coordinates or a library alias" usage error, because
    /// it is far more likely to be an address than a typo'd coordinate.
    pub fn parse(s: &str) -> Option<Self> {
        let (lat, lon) = s.split_once(',')?;
        Self::checked(lat.trim().parse().ok()?, lon.trim().parse().ok()?)
    }

    /// Great-circle distance in kilometres.
    ///
    /// Haversine on a sphere: within the region this list covers, the difference to an
    /// ellipsoidal model is a few metres over tens of kilometres, and `--near` orders
    /// houses rather than navigating to them.
    pub fn distance_km(&self, other: LatLon) -> f64 {
        let (lat1, lat2) = (self.lat.to_radians(), other.lat.to_radians());
        let d_lat = lat2 - lat1;
        let d_lon = (other.lon - self.lon).to_radians();
        let a = (d_lat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (d_lon / 2.0).sin().powi(2);
        2.0 * EARTH_RADIUS_KM * a.sqrt().clamp(0.0, 1.0).asin()
    }
}

#[cfg(test)]
mod tests {
    use super::LatLon;

    fn point(lat: f64, lon: f64) -> LatLon {
        LatLon::checked(lat, lon).expect("test coordinates are on the earth")
    }

    #[test]
    fn parses_a_coordinate_pair() {
        assert_eq!(LatLon::parse("52.52,13.39"), Some(point(52.52, 13.39)));
        assert_eq!(LatLon::parse(" 52.52 , 13.39 "), Some(point(52.52, 13.39)));
        assert_eq!(LatLon::parse("-33.9,18.4"), Some(point(-33.9, 18.4)));
    }

    /// An address is the likely input here, not a typo'd coordinate — so anything that is
    /// not two numbers has to fail cleanly and let the caller say what it wanted.
    #[test]
    fn refuses_anything_that_is_not_two_numbers() {
        for input in [
            "Unter den Linden 8",
            "52.52",
            "52.52,",
            "52.52,13.39,1",
            "north,east",
            "",
            "952.5,13.4",
        ] {
            assert!(LatLon::parse(input).is_none(), "{input:?} must not parse");
        }
    }

    #[test]
    fn distance_to_self_is_zero() {
        let hu = point(52.52041, 13.39096);
        assert!(hu.distance_km(hu).abs() < 1e-9);
    }

    /// Grimm-Zentrum to Staatsbibliothek Unter den Linden — a walk of a few minutes.
    #[test]
    fn known_short_distance() {
        let hu = point(52.52041, 13.39096);
        let stabi = point(52.51755, 13.39162);
        let km = hu.distance_km(stabi);
        assert!((0.25..0.45).contains(&km), "HU→Stabi was {km} km");
        assert!((km - stabi.distance_km(hu)).abs() < 1e-9, "not symmetric");
    }

    /// A long, well-known leg: Berlin to Potsdam over the Havel.
    #[test]
    fn known_long_distance() {
        let berlin = point(52.52041, 13.39096);
        let potsdam = point(52.39371, 13.06561);
        let km = berlin.distance_km(potsdam);
        assert!((25.5..27.0).contains(&km), "Berlin→Potsdam was {km} km");
    }
}
