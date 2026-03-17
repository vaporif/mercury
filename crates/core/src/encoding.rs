//! Traits for encoding and decoding values into byte representations.

use crate::ThreadSafe;
use crate::error::Result;

/// A codec that can encode and decode values to/from bytes.
pub trait Encoding: ThreadSafe {
    type Encoded: AsRef<[u8]> + ThreadSafe;

    fn encode<T: Encodable<Self>>(&self, value: &T) -> Result<Self::Encoded>
    where
        Self: Sized;

    fn decode<T: Decodable<Self>>(&self, data: &Self::Encoded) -> Result<T>
    where
        Self: Sized;
}

/// A type that can be encoded using a given `Encoding`.
pub trait Encodable<E: Encoding + ?Sized> {
    fn encode(&self, encoding: &E) -> Result<E::Encoded>;
}

/// A type that can be decoded from a given `Encoding`.
pub trait Decodable<E: Encoding + ?Sized>: Sized {
    fn decode(encoding: &E, data: &E::Encoded) -> Result<Self>;
}
