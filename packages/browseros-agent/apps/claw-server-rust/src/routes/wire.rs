use axum::{
    Json,
    response::{IntoResponse, Response},
};
use serde::{Serialize, Serializer, ser::Error as _};

/// Serializes through `Value` so typed handlers keep lexicographic object-key order on the wire.
pub(super) struct WireJson<T>(pub T);

impl<T: Serialize> Serialize for WireJson<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::to_value(&self.0)
            .map_err(S::Error::custom)?
            .serialize(serializer)
    }
}

impl<T: Serialize> IntoResponse for WireJson<T> {
    fn into_response(self) -> Response {
        Json(self).into_response()
    }
}
