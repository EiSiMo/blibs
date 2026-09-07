//! Distances, computed offline from the coordinates in the list.
//!
//! There is no geocoder. `--near` takes coordinates or a library's alias; free text is a
//! usage error that says so, rather than a network round trip the user did not ask for.

/// Mean earth radius in kilometres (IUGG). The list's coordinates are given to five
/// decimals, so the model's error is far below the data's.
const EARTH_RADIUS_KM: f64 = 6371.0088;

/// Why a `--near` argument is not a point.
///
/// Three cases, because they take three different remedies. The caller maps each to its
/// own usage error; nothing here formats a message, so the wording stays in `error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotAPoint {
    /// Not shaped like a coordinate pair at all: an address, a library alias, a typo.
    NotCoordinates,
    /// One number where two are needed — `52.52`, or `52.52,`.
    OnlyOneValue,
    /// Two numbers, but not a place on the earth: a latitude past ±90, a longitude past
    /// ±180, or a non-finite value.
    OutOfRange,
}

/// Whether a single fragment reads as half a coordinate pair or as text.
///
/// Split out because `52.52` and `52.52,` arrive by different routes and must land on
/// the same answer.
fn one_number_or_text(fragment: &str) -> NotAPoint {
    if fragment.trim().parse::<f64>().is_ok() {
        NotAPoint::OnlyOneValue
    } else {
        NotAPoint::NotCoordinates
    }
}

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
    /// The failure is *named* rather than collapsed into `None`, because the three ways
    /// of getting this wrong need three different answers: `91,181` was coordinates and
    /// must not be told to give coordinates, `52.52` is half a pair, and an address is
    /// the only one of the three that blibs genuinely cannot look up.
    pub fn parse(s: &str) -> Result<Self, NotAPoint> {
        let Some((lat, lon)) = s.split_once(',') else {
            return Err(one_number_or_text(s));
        };
        let (lat, lon) = (lat.trim(), lon.trim());
        if lon.is_empty() {
            // `52.52,` is the same mistake as `52.52`, typed with the comma already in.
            return Err(one_number_or_text(lat));
        }
        let (Ok(lat), Ok(lon)) = (lat.parse(), lon.parse()) else {
            return Err(NotAPoint::NotCoordinates);
        };
        Self::checked(lat, lon).ok_or(NotAPoint::OutOfRange)
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
    use super::{LatLon, NotAPoint};

    fn point(lat: f64, lon: f64) -> LatLon {
        LatLon::checked(lat, lon).expect("test coordinates are on the earth")
    }

    #[test]
    fn parses_a_coordinate_pair() {
        assert_eq!(LatLon::parse("52.52,13.39"), Ok(point(52.52, 13.39)));
        assert_eq!(LatLon::parse(" 52.52 , 13.39 "), Ok(point(52.52, 13.39)));
        assert_eq!(LatLon::parse("-33.9,18.4"), Ok(point(-33.9, 18.4)));
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
            assert!(LatLon::parse(input).is_err(), "{input:?} must not parse");
        }
    }

    /// The three failures are the whole reason this returns a `Result`: `--near 91,181`
    /// used to be answered with "give coordinates", which is no help to someone who did.
    #[test]
    fn each_way_of_failing_is_told_apart() {
        for input in ["91,181", "-91,0", "0,181", "nan,13.4"] {
            assert_eq!(
                LatLon::parse(input),
                Err(NotAPoint::OutOfRange),
                "{input:?}"
            );
        }
        for input in ["52.52", " 52.52 ", "52.52,", "-33.9,  "] {
            assert_eq!(
                LatLon::parse(input),
                Err(NotAPoint::OnlyOneValue),
                "{input:?}"
            );
        }
        for input in ["Unter den Linden 8", "STABI2", "", ",", "north,east"] {
            assert_eq!(
                LatLon::parse(input),
                Err(NotAPoint::NotCoordinates),
                "{input:?}"
            );
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
