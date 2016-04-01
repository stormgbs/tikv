use std::result::Result as StdResult;
use std::error::Error as StdError;


quick_error!{
    #[derive(Debug)]
    pub enum Error {
        Other(err: Box<StdError + Sync + Send>) {
            from()
            cause(err.as_ref())
            description(err.description())
            display("{}", err)
        }
    }
}

pub type Result<T> = StdResult<T, Error>;
