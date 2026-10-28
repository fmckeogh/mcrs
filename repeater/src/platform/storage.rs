pub trait Storage {
    fn read(&mut self, key: &str, buffer: &mut [u8]) -> Result<usize, Error>;
    fn write_atomic(&mut self, key: &str, data: &[u8]) -> Result<(), Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    NotAvailable,
    NotFound,
    BufferTooSmall,
    InvalidKey,
    Corrupt,
    Io,
}

impl embedded_storage::nor_flash::NorFlashError for Error {
    fn kind(&self) -> embedded_storage::nor_flash::NorFlashErrorKind {
        match self {
            Error::BufferTooSmall => embedded_storage::nor_flash::NorFlashErrorKind::OutOfBounds,
            _ => embedded_storage::nor_flash::NorFlashErrorKind::Other,
        }
    }
}
