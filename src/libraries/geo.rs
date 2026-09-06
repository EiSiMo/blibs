//! Distances, computed offline from the coordinates in the list.
//!
//! There is no geocoder. `--near` takes coordinates or a library's alias; free text is a
//! usage error that says so, rather than a network round trip the user did not ask for.

/// A point on the earth.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct LatLon {
    /// Latitude in degrees.
    pub lat: f64,
    /// Longitude in degrees.
    pub lon: f64,
}

impl LatLon {
    /// Parse a `lat,lon` pair as typed on the command line.
    ///
    /// Returns `None` for anything that is not two numbers separated by a comma — the
    /// caller turns that into the "coordinates or a library alias" usage error, because
    /// it is far more likely to be an address than a typo'd coordinate.
    pub fn parse(_s: &str) -> Option<Self> {
        todo!("phase 2: libraries")
    }

    /// Great-circle distance in kilometres.
    pub fn distance_km(&self, _other: LatLon) -> f64 {
        todo!("phase 2: libraries")
    }
}
