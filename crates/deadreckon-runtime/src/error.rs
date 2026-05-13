use std::path::PathBuf;

use deadreckon_core::error::{DeadreckonError, Result};

pub(crate) trait IoContext<T> {
    fn with_path(self, path: impl Into<PathBuf>) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn with_path(self, path: impl Into<PathBuf>) -> Result<T> {
        let path = path.into();
        self.map_err(|source| DeadreckonError::Io { path, source })
    }
}
