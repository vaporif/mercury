//! Traits for encoding and decoding values into byte representations.

use crate::error::Result;
use crate::ThreadSafe;

/// A codec that can encode and decode values to/from bytes.
pub trait Encoding: ThreadSafe {
    /// The encoded byte representation.
    type Encoded: AsRef<[u8]> + ThreadSafe;

    /// Encode a value into bytes.
    fn encode<T: Encodable<Self>>(&self, value: &T) -> Result<Self::Encoded>
    where
        Self: Sized;

    /// Decode a value from bytes.
    fn decode<T: Decodable<Self>>(&self, data: &Self::Encoded) -> Result<T>
    where
        Self: Sized;
}

/// A type that can be encoded using a given `Encoding`.
pub trait Encodable<E: Encoding + ?Sized> {
    /// Encode this value using the provided encoding.
    fn encode(&self, encoding: &E) -> Result<E::Encoded>;
}

/// A type that can be decoded from a given `Encoding`.
pub trait Decodable<E: Encoding + ?Sized>: Sized {
    /// Decode a value from the provided encoding and data.
    fn decode(encoding: &E, data: &E::Encoded) -> Result<Self>;
}
