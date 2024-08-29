use std::fmt::Debug;

pub trait LogErrResult<T, E: Debug> {
    /// Log an error message and return the original error.
    fn log_err(self, module_path: &str, msg: &str) -> Result<T, E>;
}

impl<T, E: Debug> LogErrResult<T, E> for Result<T, E> {
    fn log_err(self, module_path: &str, msg: &str) -> Result<T, E> {
        self.map_err(|e| {
            error!(target: module_path, "{}: {:?}", msg, e);
            e
        })
    }
}
