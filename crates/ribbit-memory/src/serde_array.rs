//! Serde support for fixed arrays longer than 32 elements (save states):
//! `#[serde(with = "ribbit_memory::serde_array")]`.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub fn serialize<S: Serializer, T: Serialize, const N: usize>(a: &[T; N], s: S) -> Result<S::Ok, S::Error> {
    s.collect_seq(a.iter())
}

pub fn deserialize<'de, D: Deserializer<'de>, T: Deserialize<'de>, const N: usize>(d: D) -> Result<[T; N], D::Error> {
    let v = Vec::<T>::deserialize(d)?;
    let n = v.len();
    v.try_into().map_err(|_| D::Error::custom(format!("expected {N} elements, got {n}")))
}
