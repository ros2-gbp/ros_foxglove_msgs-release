use std::{error::Error, fmt::Display};

use crate::FoxgloveError;

mod logging;

#[derive(Debug, thiserror::Error)]
struct SourceError(&'static str);
impl Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

#[test]
fn test_unspecified_error_downcast() {
    let src_err = SourceError("oh no");
    let fg_err = FoxgloveError::Unspecified(src_err.into());
    assert_eq!(format!("{fg_err}"), "oh no");
    assert!(fg_err
        .source()
        .unwrap()
        .downcast_ref::<SourceError>()
        .is_some());
}
