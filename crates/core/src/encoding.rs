use crate::error::Result;
use crate::ThreadSafe;

pub trait Encoding: ThreadSafe {
    type Encoded: AsRef<[u8]> + ThreadSafe;

    fn encode<T: Encodable<Self>>(&self, value: &T) -> Result<Self::Encoded>
    where
        Self: Sized;

    fn decode<T: Decodable<Self>>(&self, data: &Self::Encoded) -> Result<T>
    where
        Self: Sized;
}

pub trait Encodable<E: Encoding + ?Sized> {
    fn encode(&self, encoding: &E) -> Result<E::Encoded>;
}

pub trait Decodable<E: Encoding + ?Sized>: Sized {
    fn decode(encoding: &E, data: &E::Encoded) -> Result<Self>;
}
