pub mod encoding;
pub mod error;
pub mod runtime;
pub mod worker;

pub trait ThreadSafe: Send + Sync + 'static {}
impl<T: Send + Sync + 'static> ThreadSafe for T {}
