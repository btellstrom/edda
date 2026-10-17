//! Printing to stdout, like `println!`, except that when whatever reads
//! the output has stopped reading (`edda list | head -1`), edda stops
//! quietly instead of panicking about a broken pipe.

use std::{
    fmt,
    io::{self, Write as _},
};

/// Prints a line, like `println!`; see the module's docs.
macro_rules! say {
    () => {
        $crate::out::line(format_args!(""))
    };
    ($($arg:tt)*) => {
        $crate::out::line(format_args!($($arg)*))
    };
}

/// Prints `text` and a newline.
pub fn line(text: fmt::Arguments) {
    if let Err(e) = writeln!(io::stdout().lock(), "{text}") {
        stop(&e);
    }
}

/// Ends edda over a failed write: quietly if the reader has gone, which is
/// how a pipeline says it has seen enough.
fn stop(error: &io::Error) -> ! {
    if error.kind() == io::ErrorKind::BrokenPipe {
        std::process::exit(0);
    }
    panic!("failed printing to stdout: {error}");
}
