//! CBOR helpers. Every structure that is encrypted, signed or sent over the
//! wire goes through these so limits and error mapping stay in one place.

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{CoreError, Result};

pub fn to_vec<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    ciborium::ser::into_writer(value, &mut out).map_err(|e| CoreError::Encode(e.to_string()))?;
    Ok(out)
}

pub fn from_slice<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    ciborium::de::from_reader(bytes).map_err(|e| CoreError::Malformed(e.to_string()))
}
