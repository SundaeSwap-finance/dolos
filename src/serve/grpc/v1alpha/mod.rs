pub mod query;
pub mod submit;
pub mod sync;
pub mod watch;

pub(super) use pallas::interop::utxorpc::v1alpha::spec;

/// The value a pattern field constrains a match to, none when it is empty.
fn criterion(field: &bytes::Bytes) -> Option<&bytes::Bytes> {
    Some(field).filter(|x| !x.is_empty())
}
