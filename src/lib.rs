pub type Type<T> = std::marker::PhantomData<T>;
pub type KeyType<T> = std::marker::PhantomData<T>;
pub type RkyvType<T> = std::marker::PhantomData<T>;
pub type JsonType<T> = std::marker::PhantomData<T>;
pub type BincodeType<T> = std::marker::PhantomData<T>;
pub type MsgpackType<T> = std::marker::PhantomData<T>;

pub type MigrateError = anyhow::Error;
pub type MigrateResult<T> = anyhow::Result<T>;
pub use anyhow::anyhow as migrate_error;

pub use typeid;


#[cfg(feature = "hashed")]
pub trait MigrateableHashed {
    const TYPE_HASH_NATIVE: u128;
}

#[cfg(all(feature = "hashed", feature = "fjall"))]
impl MigrateableHashed for fjall::Slice {
    /// SHAKE128 hash of `fjall::Slice`
    const TYPE_HASH_NATIVE: u128 = 131888318210659702557404083714298940743;
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrateDataKind {
    Bytes,
    Key,
    Rkyv,
    Json,
    Bincode,
    Msgpack,
}
impl MigrateDataKind {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Bytes),
            1 => Some(Self::Key),
            2 => Some(Self::Rkyv),
            3 => Some(Self::Json),
            4 => Some(Self::Bincode),
            5 => Some(Self::Msgpack),
            _ => None,
        }
    }
}

pub trait Migrateable: Sized {
    type Current;
    type CurrentLifetimed<'c>;

    const CURRENT_DATA_KIND: MigrateDataKind;
    const CURRENT_VERSION: u8;
    fn iter() -> impl Iterator<Item = Self>;
    fn iter_migrateable() -> impl Iterator<Item = Self>;
    fn iter_u8() -> impl Iterator<Item = u8>;
    fn iter_migrateable_u8() -> impl Iterator<Item = u8>;
    fn from_u8(value: u8) -> Option<Self>;
    fn to_u8(&self) -> u8;
    fn data_kind(&self) -> MigrateDataKind;
    fn migrate<'de, I: 'de + AsRef<[u8]>, O: From<Vec<u8>>>(&self, bytes: I) -> Result<O, MigrateError>;

    #[cfg(feature = "hashed")]
    fn type_hash(&self) -> u128;
}

pub use migrateable_derive::Migrateable;

#[cfg(feature = "hashed")]
pub use hashed_type_def::HashedTypeDef;

#[cfg(feature = "hashed")]
pub use migrateable_derive::Locked;
